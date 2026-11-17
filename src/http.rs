//! HTTP client and connectivity detection.
//!
//! Only two registry hosts are ever contacted: registry.npmjs.org and
//! npm.jsr.io. nothing never pings a registry unless a package is actually
//! missing from the store, or the user explicitly asked for an update check.

use crate::json::Json;
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub const NPM_HOST: &str = "registry.npmjs.org";

#[derive(Debug)]
pub enum HttpError {
    Offline(String),
    Status { code: u16, url: String },
    Other(String),
}

impl HttpError {
    pub fn message(&self) -> String {
        match self {
            HttpError::Offline(m) => format!("offline: {m}"),
            HttpError::Status { code, url } => format!("HTTP {code} from {url}"),
            HttpError::Other(m) => m.clone(),
        }
    }
}

pub struct Client {
    agent: ureq::Agent,
}

impl Client {
    pub fn new() -> Client {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            // A tarball like next or @next/swc is 40mb; on a slow link it
            // needs more than two minutes. Transient errors still retry.
            .timeout(Duration::from_secs(600))
            .user_agent(concat!(
                "nth/",
                env!("CARGO_PKG_VERSION"),
                " (nothing package manager)"
            ))
            .build();
        Client { agent }
    }

    pub fn get_json(&self, url: &str) -> Result<Json, HttpError> {
        let bytes = self.get_bytes(url)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| HttpError::Other(format!("non-utf8 response from {url}")))?;
        Json::parse(&text).map_err(|e| HttpError::Other(format!("invalid json from {url}: {e}")))
    }

    /// GET with small retry budget for transient transport failures and 5xx.
    pub fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        let mut last_err: Option<HttpError> = None;
        for attempt in 0..3u32 {
            match self.agent.get(url).call() {
                Ok(response) => {
                    let mut buf = Vec::new();
                    response
                        .into_reader()
                        .read_to_end(&mut buf)
                        .map_err(|e| HttpError::Offline(format!("connection lost: {e}")))?;
                    return Ok(buf);
                }
                Err(ureq::Error::Status(code, _)) => {
                    let err = HttpError::Status {
                        code,
                        url: url.to_string(),
                    };
                    if code >= 500 && attempt < 2 {
                        last_err = Some(err);
                        std::thread::sleep(Duration::from_millis(400 * (attempt as u64 + 1)));
                        continue;
                    }
                    return Err(err);
                }
                Err(ureq::Error::Transport(t)) => {
                    let err = HttpError::Offline(t.to_string());
                    last_err = Some(err);
                    if attempt < 2 {
                        std::thread::sleep(Duration::from_millis(400 * (attempt as u64 + 1)));
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| HttpError::Other("request failed".into())))
    }
}

/// Cheap TCP probe. Used only to phrase messages correctly ("it appears you
/// are offline"), never to gate local-store work.
pub fn is_online() -> bool {
    let addrs = match (NPM_HOST, 443u16).to_socket_addrs() {
        Ok(a) => a,
        Err(_) => return false,
    };
    for addr in addrs.take(3) {
        if TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_ok() {
            return true;
        }
    }
    false
}
