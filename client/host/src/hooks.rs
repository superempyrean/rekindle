//! The hook points: the anchor set resolved against the client's own image,
//! and the healed set kept for the next build.

use std::collections::BTreeMap;

use crate::image::image_base;

/// Where to hook gpui's frame: `Window::draw_roots`, or (in a build that
/// inlined it into its caller) `Window::draw` (the frame is then read from
/// `rendered_frame`). `REKINDLE_FRAME_HOOK=draw` forces the fallback (to test
/// it on a build that has both). Returns the hook and its address.
pub(crate) fn frame_hook(hooks: &Hooks) -> anyhow::Result<(rekindle_ui_hook::FrameHook, usize)> {
    use rekindle_ui_hook::FrameHook;
    let forced = std::env::var("REKINDLE_FRAME_HOOK").is_ok_and(|v| v == "draw");
    match hooks.addr("gpui_window_draw_roots") {
        Ok(a) if !forced => Ok((FrameHook::DrawRoots(a), a)),
        _ => {
            let a = hooks.addr("gpui_window_draw")?;
            Ok((FrameHook::Draw(a), a))
        }
    }
}

/// The hook points, resolved: anchor name -> live address (or why not); and
/// operand anchors (`layout:<section>:<key>`) -> the value this build's code
/// carries (or why not).
pub(crate) struct Hooks {
    pub(crate) addrs: BTreeMap<String, Result<usize, String>>,
    pub(crate) values: BTreeMap<String, Result<i64, String>>,
    pub(crate) report: rekindle_anchor::ResolutionReport,
    /// The set resolved: the shipped one, or the one rekindle healed from it.
    pub(crate) set: rekindle_anchor::AnchorSet,
    /// The shipped set's sha256.
    pub(crate) base: String,
    /// The set came from the healed file.
    pub(crate) from_healed: bool,
}

/// The anchor set rekindle healed (`rekindle_anchor::heal`), kept for the
/// next build: `REKINDLE_HOST_HEALED`, or next to the shipped set. It
/// records the shipped set's sha256 and is only used with that set, so a
/// new rekindle release starts from its own anchors again.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Healed {
    pub(crate) base: String,
    pub(crate) set: rekindle_anchor::AnchorSet,
}

pub(crate) fn healed_path(anchors: &str) -> std::path::PathBuf {
    std::env::var_os("REKINDLE_HOST_HEALED")
        .map_or_else(|| format!("{anchors}.healed.json").into(), Into::into)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    crate::hex(&sha2::Sha256::digest(bytes))
}

/// Anchors this build weakened that [`rekindle_anchor::heal`] would rebuild.
pub(crate) fn healable(report: &rekindle_anchor::ResolutionReport) -> usize {
    report
        .anchors
        .iter()
        .filter(|a| a.corroborated() && a.unique_locators < a.total_locators)
        .count()
}

/// Heal what this build weakened and keep it for the next launch. Returns
/// how many anchors were healed; the file is written only if any were.
pub(crate) fn heal_for_next_build(
    set: &rekindle_anchor::AnchorSet,
    report: &rekindle_anchor::ResolutionReport,
    base: String,
    out: &std::path::Path,
) -> anyhow::Result<usize> {
    let exe = std::fs::read(crate::image::image_path()?)?;
    let h = rekindle_anchor::heal(set, report, &exe);
    if h.healed.is_empty() {
        return Ok(0);
    }
    let tmp = out.with_extension("tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&Healed { base, set: h.set })?,
    )?;
    std::fs::rename(&tmp, out)?;
    Ok(h.healed.len())
}

impl Hooks {
    /// Resolve the anchor set against the client's own image file (what the
    /// loader mapped), then map each anchor's file offset to its live
    /// address. The healed set is used instead of the shipped one when
    /// there is one for it.
    pub(crate) fn resolve(path: &str) -> anyhow::Result<Self> {
        let shipped = std::fs::read(path)?;
        let shipped_sha = sha256_hex(&shipped);
        let healed = std::fs::read(healed_path(path))
            .ok()
            .and_then(|b| serde_json::from_slice::<Healed>(&b).ok())
            .filter(|h| h.base == shipped_sha);
        let from_healed = healed.is_some();
        let set = match healed {
            Some(h) => h.set,
            None => rekindle_anchor::AnchorSet::from_json(&shipped)?,
        };
        let exe = std::fs::read(crate::image::image_path()?)?;
        let report = set.resolve(&exe);
        let pe = rekindle_sigmatch::short::PeImage::parse(&exe)?;
        let base = image_base().ok_or_else(|| anyhow::anyhow!("no image base"))?;
        let operand: std::collections::BTreeSet<&str> = set
            .anchors
            .iter()
            .filter(|a| a.operand.is_some())
            .map(|a| a.name.as_str())
            .collect();
        let mut addrs = BTreeMap::new();
        let mut values = BTreeMap::new();
        for a in &report.anchors {
            if operand.contains(a.name.as_str()) {
                let v = match (a.verdict, a.value) {
                    (rekindle_anchor::Verdict::Resolved, Some(v)) => Ok(v),
                    (verdict, _) => Err(format!("{verdict:?}").to_lowercase()),
                };
                values.insert(a.name.clone(), v);
                continue;
            }
            let addr = match a.offset() {
                Some(off) => pe
                    .offset_to_rva(off)
                    .map(|r| base + r as usize)
                    .ok_or_else(|| "offset outside sections".to_string()),
                None => Err(format!("{:?}", a.verdict).to_lowercase()),
            };
            addrs.insert(a.name.clone(), addr);
        }
        Ok(Hooks {
            addrs,
            values,
            report,
            set,
            base: shipped_sha,
            from_healed,
        })
    }

    pub(crate) fn addr(&self, name: &str) -> anyhow::Result<usize> {
        match self.addrs.get(name) {
            Some(Ok(a)) => Ok(*a),
            Some(Err(e)) => anyhow::bail!("anchor {name:?} {e}"),
            None => anyhow::bail!("no anchor {name:?}"),
        }
    }
}
