//! Host options + always-valid newtypes (parse-don't-validate).
//!
//! `HostOptions::new` takes already-validated newtypes and is
//! infallible. `HostOptions::try_new` accepts loose inputs (`&str`,
//! `&Path`) and runs every check in one place. Once constructed, every
//! field is guaranteed non-empty and well-formed.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use libp2p::Multiaddr;

use crate::error::{HostError, HostResult};

/// Listen multiaddr the host binds at `Service::start`.
///
/// Construction normalises whitespace and parses the multiaddr; the
/// inner value is therefore guaranteed to round-trip through
/// `Multiaddr::to_string` / `Multiaddr::from_str`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListenAddr(Multiaddr);

impl ListenAddr {
    /// Parses a raw multiaddr string. Empty / whitespace-only input is
    /// rejected with [`HostError::EmptyListenAddr`].
    ///
    /// # Errors
    /// - [`HostError::EmptyListenAddr`] when `input` is empty or all
    ///   whitespace.
    /// - [`HostError::InvalidListenAddr`] when libp2p rejects the
    ///   multiaddr.
    pub fn new(input: &str) -> HostResult<Self> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(HostError::EmptyListenAddr);
        }
        trimmed
            .parse::<Multiaddr>()
            .map(Self)
            .map_err(|source| HostError::InvalidListenAddr {
                input: trimmed.to_owned(),
                source,
            })
    }

    /// Borrowed view of the underlying multiaddr.
    #[must_use]
    pub fn as_multiaddr(&self) -> &Multiaddr {
        &self.0
    }

    /// Consumes the wrapper and returns the underlying multiaddr.
    #[must_use]
    pub fn into_multiaddr(self) -> Multiaddr {
        self.0
    }
}

impl fmt::Display for ListenAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Agent-version string advertised at libp2p identify handshake.
///
/// Trimmed at construction. Empty input rejected.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentVersion(String);

impl AgentVersion {
    /// Trims whitespace and rejects empty input.
    ///
    /// # Errors
    /// [`HostError::EmptyAgentVersion`] when the trimmed input is empty.
    pub fn new(input: &str) -> HostResult<Self> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(HostError::EmptyAgentVersion);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// Borrowed view of the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for AgentVersion {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// On-disk path of the host's identity key material.
///
/// Empty path rejected.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdentityPath(PathBuf);

impl IdentityPath {
    /// Rejects empty paths.
    ///
    /// # Errors
    /// [`HostError::EmptyIdentityPath`] when `path` is empty.
    pub fn new(path: impl Into<PathBuf>) -> HostResult<Self> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(HostError::EmptyIdentityPath);
        }
        Ok(Self(path))
    }

    /// Borrowed view of the underlying path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for IdentityPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

/// On-disk path of the bootnodes YAML file (flat list of multiaddr
/// strings). Empty path rejected.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BootnodesPath(PathBuf);

impl BootnodesPath {
    /// Rejects empty paths.
    ///
    /// # Errors
    /// [`HostError::EmptyBootnodesPath`] when `path` is empty.
    pub fn new(path: impl Into<PathBuf>) -> HostResult<Self> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(HostError::EmptyBootnodesPath);
        }
        Ok(Self(path))
    }

    /// Borrowed view of the underlying path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for BootnodesPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

/// Always-valid configuration for [`crate::DevnetHost::build`].
///
/// Carries an always-valid set of inputs: construction via [`Self::new`]
/// (typed) or [`Self::try_new`] (loose) guarantees every field is
/// non-empty and well-formed.
#[derive(Debug, Clone)]
#[must_use = "build a host from these options, or the configuration is dropped"]
pub struct HostOptions {
    listen_addr: ListenAddr,
    agent_version: AgentVersion,
    identity_path: IdentityPath,
    bootnodes_path: Option<BootnodesPath>,
}

impl HostOptions {
    /// Infallible constructor from already-validated newtypes.
    pub fn new(
        listen_addr: ListenAddr,
        agent_version: AgentVersion,
        identity_path: IdentityPath,
        bootnodes_path: Option<BootnodesPath>,
    ) -> Self {
        Self {
            listen_addr,
            agent_version,
            identity_path,
            bootnodes_path,
        }
    }

    /// Loose-input constructor: validates each field in one place.
    ///
    /// # Errors
    /// Forwards the first failure from [`ListenAddr::new`],
    /// [`AgentVersion::new`], [`IdentityPath::new`], or
    /// [`BootnodesPath::new`].
    pub fn try_new(
        listen_addr: &str,
        agent_version: &str,
        identity_path: &Path,
        bootnodes_path: Option<&Path>,
    ) -> HostResult<Self> {
        Ok(Self::new(
            ListenAddr::new(listen_addr)?,
            AgentVersion::new(agent_version)?,
            IdentityPath::new(identity_path)?,
            bootnodes_path.map(BootnodesPath::new).transpose()?,
        ))
    }

    /// Borrowed view of the listen multiaddr.
    #[must_use]
    pub fn listen_addr(&self) -> &ListenAddr {
        &self.listen_addr
    }

    /// Borrowed view of the agent-version string.
    #[must_use]
    pub fn agent_version(&self) -> &AgentVersion {
        &self.agent_version
    }

    /// Borrowed view of the identity-file path.
    #[must_use]
    pub fn identity_path(&self) -> &IdentityPath {
        &self.identity_path
    }

    /// Borrowed view of the bootnodes-file path (if configured).
    #[must_use]
    pub fn bootnodes_path(&self) -> Option<&BootnodesPath> {
        self.bootnodes_path.as_ref()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const VALID_LISTEN: &str = "/ip4/127.0.0.1/udp/0/quic-v1";
    const VALID_AGENT: &str = "test/0.1.0";
    const VALID_IDENTITY: &str = "/tmp/id";
    const VALID_BOOTNODES: &str = "/tmp/boot.yaml";

    #[test]
    fn try_new_happy_path() {
        let opts = HostOptions::try_new(
            VALID_LISTEN,
            VALID_AGENT,
            Path::new(VALID_IDENTITY),
            Some(Path::new(VALID_BOOTNODES)),
        )
        .unwrap();
        assert_eq!(opts.listen_addr().to_string(), VALID_LISTEN);
        assert_eq!(opts.agent_version().as_str(), VALID_AGENT);
        assert_eq!(opts.identity_path().as_path(), Path::new(VALID_IDENTITY));
        assert_eq!(
            opts.bootnodes_path().unwrap().as_path(),
            Path::new(VALID_BOOTNODES),
        );
    }

    #[test]
    fn try_new_rejects_empty_listen() {
        let err =
            HostOptions::try_new("   ", VALID_AGENT, Path::new(VALID_IDENTITY), None).unwrap_err();
        assert!(matches!(err, HostError::EmptyListenAddr), "got {err:?}");
    }

    #[test]
    fn try_new_rejects_invalid_listen() {
        let err = HostOptions::try_new(
            "not-a-multiaddr",
            VALID_AGENT,
            Path::new(VALID_IDENTITY),
            None,
        )
        .unwrap_err();
        assert!(
            matches!(err, HostError::InvalidListenAddr { .. }),
            "got {err:?}",
        );
    }

    #[test]
    fn try_new_rejects_empty_agent() {
        let err =
            HostOptions::try_new(VALID_LISTEN, " ", Path::new(VALID_IDENTITY), None).unwrap_err();
        assert!(matches!(err, HostError::EmptyAgentVersion), "got {err:?}");
    }

    #[test]
    fn try_new_rejects_empty_identity_path() {
        let err = HostOptions::try_new(VALID_LISTEN, VALID_AGENT, Path::new(""), None).unwrap_err();
        assert!(matches!(err, HostError::EmptyIdentityPath), "got {err:?}");
    }

    #[test]
    fn try_new_rejects_empty_bootnodes_path() {
        let err = HostOptions::try_new(
            VALID_LISTEN,
            VALID_AGENT,
            Path::new(VALID_IDENTITY),
            Some(Path::new("")),
        )
        .unwrap_err();
        assert!(matches!(err, HostError::EmptyBootnodesPath), "got {err:?}");
    }
}
