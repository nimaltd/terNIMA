//! Checking a server's key against OpenSSH's `known_hosts` file, which `ssh`
//! itself reads and writes too, so a server trusted in one is trusted in both.

use std::path::Path;

use russh::keys::known_hosts::{known_host_keys_path, learn_known_hosts_path};
use russh::keys::{Algorithm, HashAlg, PublicKey};

/// What the file says of a server's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// This key is in the file for this server.
    Known,
    /// The file has no key of this type for this server. `others` are the
    /// types it has: a new type can mean someone is in between.
    Unknown { others: Vec<String> },
    /// The file has another key of this type for this server, with this fingerprint.
    Changed { known: String },
}

/// The fingerprint OpenSSH shows, such as "SHA256:uNiVztksCsDhcc0u9e8BujQXVUpKZIDTMczCvj3tD2s".
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// Looks up a server's key.
pub fn check(path: &Path, host: &str, port: u16, key: &PublicKey) -> Result<Verdict, String> {
    let known = known_host_keys_path(host, port, path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    // Not the keys themselves: those compare their comments too.
    if known
        .iter()
        .any(|(_, listed)| listed.key_data() == key.key_data())
    {
        return Ok(Verdict::Known);
    }
    if let Some((_, listed)) = known
        .iter()
        .find(|(_, listed)| listed.algorithm() == key.algorithm())
    {
        return Ok(Verdict::Changed {
            known: fingerprint(listed),
        });
    }
    let mut others: Vec<String> = known
        .iter()
        .map(|(_, listed)| listed.algorithm().as_str().to_string())
        .collect();
    others.dedup();
    Ok(Verdict::Unknown { others })
}

/// Adds a server's key to the file, made if needed.
pub fn trust(path: &Path, host: &str, port: u16, key: &PublicKey) -> Result<(), String> {
    learn_known_hosts_path(host, port, key, path)
        .map_err(|error| format!("Could not save the key in {}: {error}", path.display()))
}

/// The key types to ask the server for, those the file has for it first, so a
/// server with several keys shows the one it was trusted with, as OpenSSH does.
pub fn key_types(path: &Path, host: &str, port: u16, usual: &[Algorithm]) -> Vec<Algorithm> {
    let known = known_host_keys_path(host, port, path).unwrap_or_default();
    let is_known = |algorithm: &Algorithm| {
        known.iter().any(|(_, listed)| match algorithm {
            // The file has one name for the three ways of signing with an RSA key.
            Algorithm::Rsa { .. } => listed.algorithm().is_rsa(),
            algorithm => listed.algorithm() == *algorithm,
        })
    };
    let mut types = usual.to_vec();
    types.sort_by_key(|algorithm| !is_known(algorithm));
    types
}

#[cfg(test)]
mod tests {
    use std::fs;

    use russh::keys::{EcdsaCurve, PrivateKey};

    use super::*;
    use crate::tomlfile::tests::TempDir;

    /// A new key of a type.
    fn new_key(algorithm: Algorithm) -> PublicKey {
        PrivateKey::random(&mut russh::keys::key::safe_rng(), algorithm)
            .unwrap()
            .public_key()
            .clone()
    }

    #[test]
    fn a_new_server_is_unknown_and_known_once_trusted() {
        let dir = TempDir::new("known-hosts-new");
        // Neither the file nor its folder is there yet.
        let path = dir.file(".ssh/known_hosts");
        let key = new_key(Algorithm::Ed25519);
        assert_eq!(
            check(&path, "board", 22, &key),
            Ok(Verdict::Unknown { others: Vec::new() })
        );

        trust(&path, "board", 22, &key).unwrap();
        assert_eq!(check(&path, "board", 22, &key), Ok(Verdict::Known));
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("board ssh-ed25519 ")
        );

        // Another port is another server.
        assert_eq!(
            check(&path, "board", 2222, &key),
            Ok(Verdict::Unknown { others: Vec::new() })
        );
    }

    #[test]
    fn another_key_of_the_same_type_is_a_changed_key() {
        let dir = TempDir::new("known-hosts-changed");
        let path = dir.file("known_hosts");
        let old = new_key(Algorithm::Ed25519);
        trust(&path, "board", 2222, &old).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("[board]:2222 "));
        assert_eq!(
            check(&path, "board", 2222, &new_key(Algorithm::Ed25519)),
            Ok(Verdict::Changed {
                known: fingerprint(&old)
            })
        );
    }

    #[test]
    fn a_key_of_another_type_is_unknown_and_says_what_is_known() {
        let dir = TempDir::new("known-hosts-other");
        let path = dir.file("known_hosts");
        let ed25519 = new_key(Algorithm::Ed25519);
        trust(&path, "board", 22, &ed25519).unwrap();
        let ecdsa = new_key(Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        });
        assert_eq!(
            check(&path, "board", 22, &ecdsa),
            Ok(Verdict::Unknown {
                others: vec!["ssh-ed25519".into()]
            })
        );
    }

    #[test]
    fn any_key_listed_for_the_server_is_known() {
        let dir = TempDir::new("known-hosts-both");
        let path = dir.file("known_hosts");
        let first = new_key(Algorithm::Ed25519);
        let second = new_key(Algorithm::Ed25519);
        trust(&path, "board", 22, &first).unwrap();
        trust(&path, "board", 22, &second).unwrap();
        assert_eq!(check(&path, "board", 22, &first), Ok(Verdict::Known));
        assert_eq!(check(&path, "board", 22, &second), Ok(Verdict::Known));
    }

    #[test]
    fn reads_what_openssh_writes() {
        let dir = TempDir::new("known-hosts-openssh");
        let path = dir.file("known_hosts");
        let key = new_key(Algorithm::Ed25519);
        let line = key.to_openssh().unwrap();
        // A comment, a name among others, a line for another server, and CR LF line ends.
        fs::write(
            &path,
            format!("# mine\r\nother,board,10.0.0.5 {line}\r\nelse {line}\r\n"),
        )
        .unwrap();
        assert_eq!(check(&path, "board", 22, &key), Ok(Verdict::Known));
        assert_eq!(check(&path, "10.0.0.5", 22, &key), Ok(Verdict::Known));
        assert_eq!(
            check(&path, "boar", 22, &key),
            Ok(Verdict::Unknown { others: Vec::new() })
        );
    }

    #[test]
    fn reads_a_hashed_name_as_debian_and_ubuntu_write_it() {
        let dir = TempDir::new("known-hosts-hashed");
        let path = dir.file("known_hosts");
        let line = "|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= ssh-ed25519 \
                    AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF\n";
        fs::write(&path, line).unwrap();
        let key = russh::keys::parse_public_key_base64(
            "AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF",
        )
        .unwrap();
        assert_eq!(check(&path, "example.com", 22, &key), Ok(Verdict::Known));
        assert_eq!(
            check(&path, "example.org", 22, &key),
            Ok(Verdict::Unknown { others: Vec::new() })
        );
    }

    #[test]
    fn asks_first_for_the_key_types_it_knows_for_the_server() {
        let dir = TempDir::new("known-hosts-types");
        let path = dir.file("known_hosts");
        let ecdsa = Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        };
        let usual = [Algorithm::Ed25519, ecdsa.clone()];
        assert_eq!(key_types(&path, "board", 22, &usual), usual);

        trust(&path, "board", 22, &new_key(ecdsa.clone())).unwrap();
        assert_eq!(
            key_types(&path, "board", 22, &usual),
            [ecdsa, Algorithm::Ed25519]
        );
        // Another server is not affected.
        assert_eq!(key_types(&path, "else", 22, &usual), usual);
    }
}
