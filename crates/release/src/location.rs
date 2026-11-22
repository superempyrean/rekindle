//! Where a release channel and the files it lists live: a local path or an
//! https URL (plain http only to the local machine, for testing).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// A local file, or an https URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Path(PathBuf),
    Url(url::Url),
}

impl Location {
    /// A URL if `s` starts with `https://` or `http://`, else a path. Fails
    /// on a malformed URL, or plain http to anywhere but the local machine.
    pub fn parse(s: &str) -> Result<Self> {
        if s.starts_with("https://") || s.starts_with("http://") {
            let u = url::Url::parse(s).with_context(|| format!("invalid URL {s}"))?;
            check_scheme(&u)?;
            Ok(Location::Url(u))
        } else {
            Ok(Location::Path(PathBuf::from(s)))
        }
    }

    /// `rel` resolved against this location (relative to its directory).
    pub fn join(&self, rel: &str) -> Result<Location> {
        if rel.starts_with("https://") || rel.starts_with("http://") {
            return Location::parse(rel);
        }
        match self {
            Location::Path(p) => Ok(Location::Path(
                p.parent().unwrap_or(Path::new(".")).join(rel),
            )),
            Location::Url(u) => {
                let j = u
                    .join(rel)
                    .with_context(|| format!("resolving {rel} against {u}"))?;
                check_scheme(&j)?;
                Ok(Location::Url(j))
            }
        }
    }

    /// This location with `suffix` appended (`channel.json` to
    /// `channel.json.sig`).
    pub fn with_suffix(&self, suffix: &str) -> Location {
        match self {
            Location::Path(p) => Location::Path(with_suffix(p, suffix)),
            Location::Url(u) => {
                let mut u = u.clone();
                let path = format!("{}{suffix}", u.path());
                u.set_path(&path);
                Location::Url(u)
            }
        }
    }

    /// Read it whole, refusing more than `limit` bytes.
    pub fn read(&self, limit: u64) -> Result<Vec<u8>> {
        match self {
            Location::Path(p) => {
                let len = std::fs::metadata(p)
                    .with_context(|| format!("reading {}", p.display()))?
                    .len();
                if len > limit {
                    bail!("{} is larger than {limit} bytes", p.display());
                }
                std::fs::read(p).with_context(|| format!("reading {}", p.display()))
            }
            Location::Url(u) => {
                let mut resp = ureq::get(u.as_str())
                    .call()
                    .with_context(|| format!("fetching {u}"))?;
                // ureq's limit refuses a body of exactly `limit` bytes: it
                // errors on the read after the last byte, before it sees the
                // end. The framework listing gives exact sizes.
                let bytes = resp
                    .body_mut()
                    .with_config()
                    .limit(limit.saturating_add(1))
                    .read_to_vec()
                    .with_context(|| format!("reading {u}"))?;
                if bytes.len() as u64 > limit {
                    bail!("{u} is larger than {limit} bytes");
                }
                Ok(bytes)
            }
        }
    }
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Location::Path(p) => write!(f, "{}", p.display()),
            Location::Url(u) => write!(f, "{u}"),
        }
    }
}

/// `path` with `suffix` appended to its last component.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// HTTPS only, except plain HTTP to the local machine (for testing a local channel).
fn check_scheme(u: &url::Url) -> Result<()> {
    let local = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if u.scheme() == "https" || (u.scheme() == "http" && local) {
        Ok(())
    } else {
        bail!("{u}: the channel must be fetched over https")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_and_suffixes() {
        let u = Location::parse("https://example.com/rekindle/channel.json").unwrap();
        assert_eq!(
            u.join("a.wasm").unwrap().to_string(),
            "https://example.com/rekindle/a.wasm"
        );
        assert_eq!(
            u.with_suffix(".sig").to_string(),
            "https://example.com/rekindle/channel.json.sig"
        );
        assert!(Location::parse("http://example.com/c.json").is_err());
        assert!(Location::parse("http://127.0.0.1:8080/c.json").is_ok());
        let p = Location::parse("dist/channel.json").unwrap();
        assert_eq!(
            p.join("framework/1.0.0/x.dll").unwrap(),
            Location::Path(PathBuf::from("dist/framework/1.0.0/x.dll"))
        );
    }

    /// Serve `body` once on 127.0.0.1 and return its URL.
    fn serve_once(body: &'static [u8]) -> Location {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = s.read(&mut req);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(body);
        });
        Location::parse(&format!("http://127.0.0.1:{port}/f")).unwrap()
    }

    #[test]
    fn a_url_reads_up_to_its_limit_and_no_further() {
        assert_eq!(
            serve_once(b"12345").read(5).unwrap(),
            b"12345",
            "exactly the limit"
        );
        assert_eq!(serve_once(b"12345").read(6).unwrap(), b"12345");
        assert!(serve_once(b"123456").read(5).is_err());
    }
}
