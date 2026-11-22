//! Fork-point search: which commit of a source repository a binary was built
//! from, or a fork of it branched from, read off its panic locations.
//!
//! Each panic location names a file, a line and a column. At the commit the
//! binary was built from, that column lands on a panic site (see
//! [`locations::is_site`]). At other commits, edits have moved the lines, so
//! fewer of them do. The search scores every candidate commit by the share of
//! locations that land on a site and ranks them. Nothing is compiled: each
//! commit costs a `git cat-file` per file whose content changed.
//!
//! A fork scores below 100% at its base, and the files it edited score lowest.
//! Where the fork inserted or removed lines, the locations after the edit
//! land on sites again once shifted by that many lines. [`edits`] finds those
//! shifts and where they start. A type defined inside an edit's range is one
//! whose layout can't be taken from the base.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::locations::{self, PanicLocation};

/// The commits that share one score. Commits that change none of the scored
/// files can't be told apart, so the answer is often a run of them.
#[derive(Debug, Clone, Serialize)]
pub struct CommitScore {
    /// The newest commit with this score, in `git rev-list` order.
    pub commit: String,
    pub date: String,
    pub subject: String,
    /// Locations that land on a panic site at these commits.
    pub hits: usize,
    /// How many of the scored commits have this score.
    pub ties: usize,
    /// The oldest of them.
    pub oldest: String,
}

/// How one file's locations agree with a commit.
#[derive(Debug, Clone, Serialize)]
pub struct FileScore {
    /// Repository-relative path.
    pub path: String,
    pub locations: usize,
    pub hits: usize,
    /// Where the binary's copy of the file has lines the commit doesn't, or
    /// the other way round.
    pub edits: Vec<Edit>,
}

/// Lines inserted or removed between two lines of the base file. The edit is
/// somewhere after `after` and before `before` (base line numbers, the
/// nearest panic locations on either side).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Edit {
    pub after: u32,
    pub before: u32,
    /// Lines the binary's copy has more (negative: fewer).
    pub inserted: i64,
    /// The structs, enums and unions whose definitions overlap the range:
    /// the types whose layout the edit may have changed.
    pub types: Vec<String>,
}

/// `(name, first line, last line)` of each struct, enum and union defined in
/// `lines`, 1-based.
fn type_spans(lines: &[&str]) -> Vec<(String, u32, u32)> {
    let def = regex::Regex::new(
        r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|union)\s+([A-Za-z_][A-Za-z0-9_]*)",
    )
    .expect("the pattern is valid");
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let Some(m) = def.captures(l) else { continue };
        // The definition ends where its braces balance, or at a `;` before
        // any brace (a unit or tuple struct).
        let mut depth = 0i32;
        let mut end = i;
        'scan: for (j, t) in lines.iter().enumerate().skip(i) {
            for c in t.chars() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = j;
                            break 'scan;
                        }
                    }
                    ';' if depth == 0 => {
                        end = j;
                        break 'scan;
                    }
                    _ => {}
                }
            }
        }
        out.push((m[1].to_string(), i as u32 + 1, end as u32 + 1));
    }
    out
}

/// Wrappers whose size doesn't depend on their type argument: a type behind
/// one of these isn't held by value.
const INDIRECT: &[&str] = &[
    "Box",
    "Rc",
    "Arc",
    "Weak",
    "Vec",
    "VecDeque",
    "HashMap",
    "HashSet",
    "BTreeMap",
    "BTreeSet",
    "FxHashMap",
    "FxHashSet",
    "IndexMap",
    "IndexSet",
    "PhantomData",
    "Entity",
    "WeakEntity",
    "Task",
];

/// Whether the type name at byte `at` of a field's line is held by value:
/// not behind a reference, a raw pointer or an [`INDIRECT`] wrapper.
fn by_value(line: &str, at: usize) -> bool {
    let mut before = line[..at].trim_end();
    for word in ["mut", "const"] {
        before = before.strip_suffix(word).map_or(before, str::trim_end);
    }
    // `&'a T`: drop the lifetime.
    if let Some((head, tail)) = before.rsplit_once('\'')
        && tail.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        before = head.trim_end();
    }
    if before.ends_with('&') || before.ends_with('*') {
        return false;
    }
    // Walk out through the enclosing `<`s; any indirect wrapper makes it so.
    let b = line.as_bytes();
    let mut depth = 0i32;
    for i in (0..at).rev() {
        match b[i] {
            b'>' => depth += 1,
            b'<' if depth > 0 => depth -= 1,
            b'<' => {
                let head = &line[..i];
                let start = head
                    .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .map_or(0, |j| j + 1);
                if INDIRECT.contains(&&head[start..]) {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

/// `seeds` and every type that holds one of them by value, transitively,
/// among the structs, enums and unions defined in `sources`. A type that
/// holds an edited type moves with it: `Div` holds an `Interactivity`.
/// Names are compared without paths, so two types that share a name are
/// both taken.
pub fn containing(sources: &[String], seeds: &[String]) -> Vec<String> {
    let ident = regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").expect("the pattern is valid");
    // (name, the lines of its definition, comments cut)
    let mut defs: Vec<(String, Vec<String>)> = Vec::new();
    for text in sources {
        let lines: Vec<&str> = text.lines().collect();
        for (name, start, end) in type_spans(&lines) {
            let body = (start as usize - 1..end as usize)
                .map(|i| lines[i].split("//").next().unwrap_or("").to_string())
                .collect();
            defs.push((name, body));
        }
    }
    let known: HashSet<&str> = defs.iter().map(|(n, _)| n.as_str()).collect();
    let mut holds: Vec<(&str, HashSet<&str>)> = Vec::new();
    for (name, body) in &defs {
        let mut inner = HashSet::new();
        for (k, line) in body.iter().enumerate() {
            if line.trim_start().starts_with("#[") {
                continue;
            }
            for m in ident.find_iter(line) {
                let w = m.as_str();
                if let Some(&t) = known.get(w)
                    && !(k == 0 && w == name)
                    && by_value(line, m.start())
                {
                    inner.insert(t);
                }
            }
        }
        holds.push((name.as_str(), inner));
    }
    let mut out: HashSet<String> = seeds.iter().cloned().collect();
    loop {
        let before = out.len();
        for (name, inner) in &holds {
            if !out.contains(*name) && inner.iter().any(|t| out.contains(*t)) {
                out.insert(name.to_string());
            }
        }
        if out.len() == before {
            break;
        }
    }
    let mut out: Vec<String> = out.into_iter().collect();
    out.sort();
    out
}

/// The line edits that explain where `locs` (the binary's `(line, col)` in
/// one file) miss the sites of `lines` (that file at the base commit).
///
/// Each location is assigned a shift such that `line + shift` lands on a
/// site, from the shifts that explain the most misses. A change of shift
/// costs as much as two and a half locations explain, so a stray location
/// that happens to land on a site doesn't make an edit. Locations that no
/// shift explains keep the shift around them.
pub fn edits(lines: &[&str], locs: &[(u32, u32)]) -> Vec<Edit> {
    /// The largest shift tried, in lines.
    const RANGE: i64 = 500;
    /// What a change of shift costs, in doubled locations: two and a half
    /// locations, kept an integer.
    const SWITCH: i64 = 5;
    let site = |l: u32, c: u32, d: i64| -> bool {
        let b = i64::from(l) + d;
        b >= 1
            && lines
                .get(b as usize - 1)
                .is_some_and(|t| locations::is_site(t, c))
    };
    let mut locs = locs.to_vec();
    locs.sort_unstable();
    locs.dedup();
    if locs.is_empty() {
        return Vec::new();
    }
    let mut counts: HashMap<i64, usize> = HashMap::new();
    for &(l, c) in &locs {
        if !site(l, c, 0) {
            for d in (-RANGE..=RANGE).filter(|&d| d != 0 && site(l, c, d)) {
                *counts.entry(d).or_default() += 1;
            }
        }
    }
    let mut shifts: Vec<(i64, usize)> = counts.into_iter().filter(|&(_, n)| n >= 3).collect();
    shifts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.abs().cmp(&b.0.abs())));
    let mut cand = vec![0];
    cand.extend(shifts.iter().take(6).map(|&(d, _)| d));

    // dp over the locations in line order, a location on a site scoring 2.
    let k = cand.len();
    let mut score = vec![0i64; k];
    let mut from: Vec<Vec<usize>> = Vec::with_capacity(locs.len());
    for (i, &(l, c)) in locs.iter().enumerate() {
        let mut next = vec![0i64; k];
        let mut back = vec![0usize; k];
        for s in 0..k {
            let gain = if site(l, c, cand[s]) { 2 } else { 0 };
            if i == 0 {
                next[s] = gain - if s == 0 { 0 } else { SWITCH };
                continue;
            }
            let (best, arg) = (0..k)
                .map(|p| (score[p] - if p == s { 0 } else { SWITCH }, p))
                .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
                .expect("shift 0 is always a candidate");
            next[s] = best + gain;
            back[s] = arg;
        }
        score = next;
        from.push(back);
    }
    let mut path = vec![0usize; locs.len()];
    path[locs.len() - 1] = (0..k)
        .max_by(|&a, &b| score[a].cmp(&score[b]).then(b.cmp(&a)))
        .expect("shift 0 is always a candidate");
    for i in (1..locs.len()).rev() {
        path[i - 1] = from[i][path[i]];
    }
    // An edit's range runs between the nearest locations that only one of the
    // candidate shifts puts on a site. A location on no site, or on a site
    // under two shifts (one of them by chance), says nothing about where the
    // edit is.
    let at = |i: usize, d: i64| (i64::from(locs[i].0) + d) as u32;
    let on = |i: usize, d: i64| {
        site(locs[i].0, locs[i].1, d)
            && cand
                .iter()
                .filter(|&&e| site(locs[i].0, locs[i].1, e))
                .count()
                == 1
    };
    let spans = type_spans(lines);
    let types = |after: u32, before: u32| -> Vec<String> {
        spans
            .iter()
            .filter(|(_, s, e)| *s <= before && *e >= after)
            .map(|(n, _, _)| n.clone())
            .collect()
    };
    let mut out = Vec::new();
    let (mut cur, mut floor) = (0i64, 0u32);
    let mut seg: Vec<usize> = Vec::new(); // locations on a site under `cur`
    for i in 0..locs.len() {
        let d = cand[path[i]];
        if on(i, cur) {
            seg.push(i);
            continue;
        }
        if d == cur || !on(i, d) {
            continue;
        }
        let after = seg.last().map_or(floor, |&j| at(j, cur));
        let before = at(i, d);
        out.push(Edit {
            after,
            before,
            inserted: cur - d,
            types: types(after, before),
        });
        (cur, floor, seg) = (d, before, vec![i]);
    }
    out
}

/// The result of [`search`].
#[derive(Debug, Clone, Serialize)]
pub struct ForkPoint {
    /// Locations whose file is in the repository.
    pub locations: usize,
    pub files: usize,
    pub commits_scored: usize,
    /// Best first.
    pub ranked: Vec<CommitScore>,
    /// Per file at the best commit, lowest agreement first.
    pub best_files: Vec<FileScore>,
    /// `debug_assert` calls in the scored files at the best commit, and how
    /// many of them have a panic location. The binary has those only with
    /// debug assertions on, which also changes the layout of every type with
    /// a `#[cfg(debug_assertions)]` field.
    pub debug_asserts: usize,
    pub debug_asserts_located: usize,
    /// The types an edit's range overlaps, and every type that holds one of
    /// them by value, in the crates the scored files belong to: the types
    /// whose layout can't be taken from the best commit.
    pub suspect_types: Vec<String>,
}

fn git(repo: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(repo);
    c
}

fn git_out(repo: &Path, args: &[&str]) -> Result<String> {
    let out = git(repo).args(args).output().context("running git")?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Map each binary path to the repository path it ends with, e.g.
/// `C:\..\c84af5b\crates\gpui\src\window.rs` to `crates/gpui/src/window.rs`.
/// The longest matching suffix wins.
fn map_paths(locs: &[PanicLocation], tree: &HashSet<String>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for l in locs {
        if map.contains_key(&l.file) {
            continue;
        }
        let norm = l.file.replace('\\', "/");
        let mut rest = norm.as_str();
        loop {
            if tree.contains(rest) {
                map.insert(l.file.clone(), rest.to_string());
                break;
            }
            match rest.find('/') {
                Some(i) => rest = &rest[i + 1..],
                None => break,
            }
        }
    }
    map
}

/// A running `git cat-file`, its answers read in the order of its queries.
struct CatFile {
    child: Child,
    out: BufReader<ChildStdout>,
    line: String,
}

impl CatFile {
    /// Starts `git cat-file <mode>` over `queries`, writing them from a
    /// thread so a long batch can't fill both pipes.
    fn start(repo: &Path, mode: &str, queries: Vec<String>) -> Result<Self> {
        let mut child = git(repo)
            .args(["cat-file", mode])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .context("starting git cat-file")?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        std::thread::spawn(move || {
            for q in queries {
                if writeln!(stdin, "{q}").is_err() {
                    break;
                }
            }
        });
        let out = BufReader::new(child.stdout.take().expect("stdout is piped"));
        Ok(Self {
            child,
            out,
            line: String::new(),
        })
    }

    /// The next answer's header line.
    fn header(&mut self) -> Result<&str> {
        self.line.clear();
        if self.out.read_line(&mut self.line)? == 0 {
            bail!("git cat-file stopped before answering every query");
        }
        Ok(self.line.trim_end())
    }

    /// The next `--batch-check` answer: the object's id if it's a blob.
    fn next_blob_id(&mut self) -> Result<Option<String>> {
        let mut parts = self.header()?.split_whitespace();
        let oid = parts.next().unwrap_or_default();
        Ok((parts.next() == Some("blob")).then(|| oid.to_string()))
    }

    /// The next `--batch` answer's contents.
    fn next_contents(&mut self) -> Result<String> {
        let header = self.header()?;
        let size: usize = header
            .split_whitespace()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .with_context(|| format!("git cat-file answered {header:?}"))?;
        // The contents, then a newline.
        let mut buf = vec![0; size + 1];
        self.out.read_exact(&mut buf)?;
        buf.truncate(size);
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// Waits for git to exit. Fails if it failed.
    fn finish(mut self) -> Result<()> {
        let status = self.child.wait().context("waiting for git cat-file")?;
        if !status.success() {
            bail!("git cat-file failed ({status})");
        }
        Ok(())
    }
}

/// The blob of every scored file at every candidate commit. Blob ids are
/// interned: commits x files can run to millions.
struct Blobs {
    /// The distinct blob ids.
    oids: Vec<String>,
    /// `ids[commit * files + file]`: an index into `oids`, or
    /// [`Blobs::NONE`] where the file doesn't exist at that commit.
    ids: Vec<u32>,
    files: usize,
}

impl Blobs {
    const NONE: u32 = u32::MAX;

    fn read(repo: &Path, commits: &[String], files: &[String]) -> Result<Self> {
        let queries = commits
            .iter()
            .flat_map(|c| files.iter().map(move |f| format!("{c}:{f}")))
            .collect();
        let mut git = CatFile::start(repo, "--batch-check", queries)?;
        let mut oids: Vec<String> = Vec::new();
        let mut index: HashMap<String, u32> = HashMap::new();
        let mut ids = Vec::with_capacity(commits.len() * files.len());
        for _ in 0..commits.len() * files.len() {
            ids.push(match git.next_blob_id()? {
                Some(oid) => *index.entry(oid).or_insert_with_key(|oid| {
                    oids.push(oid.clone());
                    (oids.len() - 1) as u32
                }),
                None => Self::NONE,
            });
        }
        git.finish()?;
        Ok(Self {
            oids,
            ids,
            files: files.len(),
        })
    }

    /// The blob of file `fi` at commit `ci`, if the file exists there.
    fn at(&self, ci: usize, fi: usize) -> Option<u32> {
        let id = self.ids[ci * self.files + fi];
        (id != Self::NONE).then_some(id)
    }
}

/// Line `line` (1-based) of `lines`.
fn line_at<'a>(lines: &[&'a str], line: u32) -> Option<&'a str> {
    lines.get((line as usize).checked_sub(1)?).copied()
}

/// How many of `locs` land on a panic site in `text`.
fn site_hits(text: &str, locs: &[(u32, u32)]) -> usize {
    let lines: Vec<&str> = text.lines().collect();
    locs.iter()
        .filter(|&&(l, c)| line_at(&lines, l).is_some_and(|t| locations::is_site(t, c)))
        .count()
}

/// Score the commits `git rev-list <rev_args>` lists in `repo` against the
/// binary's panic locations.
pub fn search(
    repo: &Path,
    locs: &[PanicLocation],
    rev_args: &[String],
    top: usize,
) -> Result<ForkPoint> {
    let mut args = vec!["rev-list"];
    args.extend(rev_args.iter().map(String::as_str));
    let commits: Vec<String> = git_out(repo, &args)?.lines().map(str::to_string).collect();
    let (Some(newest), Some(oldest)) = (commits.first(), commits.last()) else {
        bail!("git rev-list {} lists no commits", rev_args.join(" "));
    };

    // Paths that exist at the newest or the oldest candidate.
    let mut tree = HashSet::new();
    for c in [newest, oldest] {
        tree.extend(
            git_out(repo, &["ls-tree", "-r", "--name-only", c])?
                .lines()
                .map(str::to_string),
        );
    }
    let map = map_paths(locs, &tree);
    // Repository path -> its locations' (line, col).
    let mut by_file: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    for l in locs {
        if let Some(p) = map.get(&l.file) {
            by_file.entry(p.clone()).or_default().push((l.line, l.col));
        }
    }
    let files: Vec<String> = by_file.keys().cloned().collect();
    let used: usize = by_file.values().map(Vec::len).sum();
    if used == 0 {
        bail!(
            "none of the binary's panic locations name a file in {}",
            repo.display()
        );
    }

    let blobs = Blobs::read(repo, &commits, &files)?;
    let hits_of = score_blobs(repo, &blobs, &files, &by_file)?;
    let file_hits = |ci: usize, fi: usize| blobs.at(ci, fi).map_or(0, |o| hits_of[&(fi, o)]);
    // Score -> the commits with it, in rev-list order (newest first).
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for ci in 0..commits.len() {
        groups
            .entry((0..files.len()).map(|fi| file_hits(ci, fi)).sum())
            .or_default()
            .push(ci);
    }
    let order: Vec<(usize, Vec<usize>)> = groups.into_iter().rev().take(top.max(1)).collect();
    let ranked = describe(repo, &commits, &order)?;

    let best = order[0].1[0];
    let best_hits: Vec<usize> = (0..files.len()).map(|fi| file_hits(best, fi)).collect();
    let inspected = inspect_best(repo, &blobs, best, &files, &by_file, &best_hits)?;
    let mut best_files = inspected.files;
    let seeds: Vec<String> = best_files
        .iter()
        .flat_map(|f| f.edits.iter().flat_map(|e| e.types.iter().cloned()))
        .collect();
    let suspect_types = if seeds.is_empty() {
        Vec::new()
    } else {
        suspect_types(repo, &commits[best], &files, &seeds)?
    };
    best_files.sort_by(|a, b| {
        (a.hits as f64 / a.locations as f64)
            .total_cmp(&(b.hits as f64 / b.locations as f64))
            .then(b.locations.cmp(&a.locations))
    });

    Ok(ForkPoint {
        locations: used,
        files: files.len(),
        commits_scored: commits.len(),
        ranked,
        best_files,
        debug_asserts: inspected.debug_asserts,
        debug_asserts_located: inspected.debug_asserts_located,
        suspect_types,
    })
}

/// The locations that land on a site, for each distinct (file, blob).
fn score_blobs(
    repo: &Path,
    blobs: &Blobs,
    files: &[String],
    by_file: &BTreeMap<String, Vec<(u32, u32)>>,
) -> Result<HashMap<(usize, u32), usize>> {
    let mut wanted: Vec<(usize, u32)> = Vec::new();
    let mut seen = HashSet::new();
    for (i, &b) in blobs.ids.iter().enumerate() {
        let fi = i % files.len();
        if b != Blobs::NONE && seen.insert((fi, b)) {
            wanted.push((fi, b));
        }
    }
    let queries = wanted
        .iter()
        .map(|&(_, o)| blobs.oids[o as usize].clone())
        .collect();
    let mut git = CatFile::start(repo, "--batch", queries)?;
    let mut hits = HashMap::new();
    for &(fi, oid) in &wanted {
        let text = git.next_contents()?;
        hits.insert((fi, oid), site_hits(&text, &by_file[&files[fi]]));
    }
    git.finish()?;
    Ok(hits)
}

/// A [`CommitScore`] for each score in `order`, with the newest commit's
/// date and subject from `git log`.
fn describe(
    repo: &Path,
    commits: &[String],
    order: &[(usize, Vec<usize>)],
) -> Result<Vec<CommitScore>> {
    let mut args = vec!["log", "--no-walk=unsorted", "--format=%H%x09%cs%x09%s"];
    args.extend(order.iter().map(|(_, cis)| commits[cis[0]].as_str()));
    let meta: HashMap<String, (String, String)> = git_out(repo, &args)?
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            Some((
                p.next()?.to_string(),
                (p.next()?.to_string(), p.next().unwrap_or("").to_string()),
            ))
        })
        .collect();
    Ok(order
        .iter()
        .map(|(hits, cis)| {
            let (date, subject) = meta.get(&commits[cis[0]]).cloned().unwrap_or_default();
            CommitScore {
                commit: commits[cis[0]].clone(),
                date,
                subject,
                hits: *hits,
                ties: cis.len(),
                oldest: commits[cis[cis.len() - 1]].clone(),
            }
        })
        .collect())
}

/// What the scored files say at the best commit.
struct Inspected {
    /// Per file, in `files` order, with the edits of each file that misses.
    files: Vec<FileScore>,
    debug_asserts: usize,
    debug_asserts_located: usize,
}

/// Locates the edits in the files that miss at commit `best`, and counts
/// the debug assertions in the files.
fn inspect_best(
    repo: &Path,
    blobs: &Blobs,
    best: usize,
    files: &[String],
    by_file: &BTreeMap<String, Vec<(u32, u32)>>,
    best_hits: &[usize],
) -> Result<Inspected> {
    let mut scores: Vec<FileScore> = files
        .iter()
        .zip(best_hits)
        .map(|(f, &hits)| FileScore {
            path: f.clone(),
            locations: by_file[f].len(),
            hits,
            edits: Vec::new(),
        })
        .collect();
    let present: Vec<(usize, u32)> = (0..files.len())
        .filter_map(|fi| Some((fi, blobs.at(best, fi)?)))
        .collect();
    let queries = present
        .iter()
        .map(|&(_, o)| blobs.oids[o as usize].clone())
        .collect();
    let mut git = CatFile::start(repo, "--batch", queries)?;
    let (mut debug_asserts, mut debug_asserts_located) = (0, 0);
    for &(fi, _) in &present {
        let text = git.next_contents()?;
        let lines: Vec<&str> = text.lines().collect();
        let locs = &by_file[&files[fi]];
        debug_asserts += text.matches("debug_assert").count();
        debug_asserts_located += locs
            .iter()
            .filter(|&&(l, c)| {
                line_at(&lines, l)
                    .and_then(|t| from_column(t, c))
                    .is_some_and(|t| t.starts_with("debug_assert"))
            })
            .count();
        if best_hits[fi] < locs.len() {
            scores[fi].edits = edits(&lines, locs);
        }
    }
    git.finish()?;
    Ok(Inspected {
        files: scores,
        debug_asserts,
        debug_asserts_located,
    })
}

/// `text` from column `col` (1-based, in characters) on.
fn from_column(text: &str, col: u32) -> Option<&str> {
    let i = (col as usize).checked_sub(1)?;
    let (at, _) = text.char_indices().nth(i)?;
    Some(&text[at..])
}

/// `seeds` and the types that hold them by value, among every source file
/// at `commit` of the crates `files` belong to.
fn suspect_types(
    repo: &Path,
    commit: &str,
    files: &[String],
    seeds: &[String],
) -> Result<Vec<String>> {
    let crates: HashSet<&str> = files
        .iter()
        .filter_map(|f| f.find("/src/").map(|i| &f[..i + 5]))
        .collect();
    let mut paths = Vec::new();
    for c in crates {
        paths.extend(
            git_out(repo, &["ls-tree", "-r", "--name-only", commit, "--", c])?
                .lines()
                .filter(|p| {
                    Path::new(p)
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("rs"))
                })
                .map(|p| format!("{commit}:{p}")),
        );
    }
    let n = paths.len();
    let mut git = CatFile::start(repo, "--batch", paths)?;
    let sources = (0..n)
        .map(|_| git.next_contents())
        .collect::<Result<Vec<_>>>()?;
    git.finish()?;
    Ok(containing(&sources, seeds))
}

#[cfg(test)]
mod tests {
    use super::{Edit, edits, from_column, search, site_hits};
    use crate::locations::PanicLocation;

    #[test]
    fn search_ranks_the_commit_the_locations_fit_first() {
        let repo = std::env::temp_dir().join(format!("rekindle-fork-point-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .expect("git runs");
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap()
        };
        git(&["init", "-q"]);
        let body = "fn f() {\n    a.unwrap();\n    b.unwrap();\n}\n";
        std::fs::write(repo.join("src/lib.rs"), body).unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "built"]);
        let built = git(&["rev-parse", "HEAD"]).trim().to_string();
        std::fs::write(repo.join("src/lib.rs"), format!("// moved\n{body}")).unwrap();
        git(&["commit", "-q", "-am", "later"]);

        // The binary's copy is the first commit's: `unwrap` at column 7 of
        // lines 2 and 3.
        let locs: Vec<PanicLocation> = [(2, 7), (3, 7)]
            .map(|(line, col)| PanicLocation {
                file: r"C:\build\x\src\lib.rs".into(),
                line,
                col,
            })
            .into();
        let fp = search(&repo, &locs, &["--all".to_string()], 5).unwrap();
        let _ = std::fs::remove_dir_all(&repo);
        assert_eq!((fp.locations, fp.files, fp.commits_scored), (2, 1, 2));
        assert_eq!(fp.ranked[0].commit, built);
        assert_eq!(fp.ranked[0].hits, 2);
        // At the later commit, line 3 is the first commit's line 2.
        assert_eq!(fp.ranked[1].hits, 1);
    }

    #[test]
    fn line_and_column_zero_land_nowhere() {
        assert_eq!(site_hits("x.unwrap();", &[(0, 3), (1, 0), (1, 3)]), 1);
        assert_eq!(from_column("abc", 0), None);
        assert_eq!(from_column("abc", 2), Some("bc"));
        assert_eq!(from_column("abc", 4), None);
    }

    #[test]
    fn an_inserted_line() {
        // Base line `b` has a call at column `b + 3`, so no other line has
        // one there. The binary's copy has one extra line after base line 10
        // and two fewer after base line 30.
        let base: Vec<String> = (1..=40)
            .map(|b| format!("{}x.unwrap();", " ".repeat(b)))
            .collect();
        let lines: Vec<&str> = base.iter().map(String::as_str).collect();
        let mut locs = Vec::new();
        for b in (2..=40).step_by(2) {
            let l = match b {
                ..=10 => b,
                11..=30 => b + 1,
                31..=32 => continue, // removed
                _ => b - 1,
            };
            locs.push((l, b + 3)); // `unwrap`
        }
        assert_eq!(
            edits(&lines, &locs),
            [
                Edit {
                    after: 10,
                    before: 12,
                    inserted: 1,
                    types: vec![]
                },
                Edit {
                    after: 30,
                    before: 34,
                    inserted: -2,
                    types: vec![]
                }
            ]
        );
        assert_eq!(edits(&lines, &[(4, 7), (8, 11)]), []);
    }

    #[test]
    fn containing() {
        let src = "pub struct Div {\n    interactivity: Interactivity,\n    children: Vec<AnyElement>,\n}\n\
                   struct Interactivity { a: u8 }\n\
                   struct Holder { d: Option<Div> }\n\
                   struct ByRef<'a> { d: &'a Div, e: Box<Option<Holder>>, f: Rc<Interactivity> }\n\
                   struct AnyElement;";
        assert_eq!(
            super::containing(&[src.to_string()], &["Interactivity".to_string()]),
            ["Div", "Holder", "Interactivity"]
        );
    }

    #[test]
    fn type_spans() {
        let src = [
            "pub struct Window {",
            "    a: u8,",
            "}",
            "struct Unit;",
            "pub(crate) enum E { A, B }",
            "fn f() {}",
        ];
        assert_eq!(
            super::type_spans(&src),
            [
                ("Window".to_string(), 1, 3),
                ("Unit".to_string(), 4, 4),
                ("E".to_string(), 5, 5)
            ]
        );
    }
}
