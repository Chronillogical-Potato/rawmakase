//! The contract between RAWmakase and the clients that control it: the
//! `rawmakase control` command, `rawmakase-ctl` and the MCP server.
//!
//! The app listens on a loopback socket and announces it in [`Endpoint`], a
//! `control.json` file in its data folder ([`paths::data_dir`]). Clients read
//! that file, connect, and send one JSON request per line carrying
//! [`PROTOCOL`] and the endpoint's token.
//!
//! This crate depends on nothing of the app's, so a client builds without its
//! GUI, GPU or native decoding libraries.
pub mod paths;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The version of the request and reply format. Both sides refuse any other.
pub const PROTOCOL: u32 = 1;

/// The name of the file announcing the socket, in the app's data folder.
pub const ENDPOINT_FILE: &str = "control.json";

/// Where the running app listens, as `control.json` holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub protocol: u32,
    /// A loopback TCP port.
    pub port: u16,
    /// Sent with every request; only someone who can read the file knows it.
    pub token: String,
    /// The app's process, so a stale file can be told from a live one. Clients
    /// need only the port and token, so they accept a file without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}
impl Endpoint {
    /// The endpoint file in `data_dir`.
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(ENDPOINT_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The field names `control.json` has always had: older clients read them.
    #[test]
    fn the_endpoint_file_keeps_its_format() {
        let endpoint = Endpoint {
            protocol: PROTOCOL,
            port: 51234,
            token: "secret".into(),
            pid: Some(42),
        };
        let json = serde_json::to_string(&endpoint).unwrap();
        assert_eq!(
            json,
            r#"{"protocol":1,"port":51234,"token":"secret","pid":42}"#
        );
        assert_eq!(serde_json::from_str::<Endpoint>(&json).unwrap(), endpoint);
        let without_pid = r#"{"protocol":1,"port":51234,"token":"secret"}"#;
        assert_eq!(
            serde_json::from_str::<Endpoint>(without_pid).unwrap().pid,
            None
        );
    }
}
