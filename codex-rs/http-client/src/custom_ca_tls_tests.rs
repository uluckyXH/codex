//! Handshake coverage for the custom trust path retained by Windows realtime connections.

use super::CODEX_CA_CERT_ENV;
use super::ConfiguredCaBundle;
use super::EnvSource;
use super::build_reqwest_client_with_env;
use super::build_rustls_client_config;
use super::maybe_build_rustls_client_config_with_env;
use pretty_assertions::assert_eq;
use rcgen::BasicConstraints;
use rcgen::CertificateParams;
use rcgen::CertifiedIssuer;
use rcgen::IsCa;
use rcgen::KeyPair;
use std::sync::Arc;

struct HarmonyTestEnv(std::path::PathBuf);

impl EnvSource for HarmonyTestEnv {
    fn var(&self, _key: &str) -> Option<String> {
        None
    }

    fn platform_ca_file(&self) -> Option<std::path::PathBuf> {
        Some(self.0.clone())
    }
}

#[tokio::test]
async fn harmony_default_bundle_is_shared_and_reloads_rotated_roots() {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let root = CertifiedIssuer::self_signed(params.clone(), KeyPair::generate().unwrap()).unwrap();
    let replacement = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&leaf_key, &root)
        .unwrap();
    let temp = tempfile::TempDir::new().unwrap();
    let env = HarmonyTestEnv(temp.path().join("system-ca.pem"));
    std::fs::write(&env.0, root.pem()).unwrap();

    // Both transports must accept the platform selection without an env override.
    let _http = build_reqwest_client_with_env(&env, reqwest::Client::builder().no_proxy()).unwrap();
    let client = maybe_build_rustls_client_config_with_env(&env)
        .unwrap()
        .expect("OHOS trust config");
    let server = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![leaf.der().clone()], leaf_key.into())
            .unwrap(),
    );
    assert_eq!(
        handshake(client.clone(), server.clone(), "localhost"),
        Ok(())
    );
    assert!(matches!(
        handshake(client, server.clone(), "wrong.example"),
        Err(rustls::Error::InvalidCertificate(_))
    ));

    // A newly built client must observe the system update and reject the old root.
    std::fs::write(&env.0, replacement.pem()).unwrap();
    let client = maybe_build_rustls_client_config_with_env(&env)
        .unwrap()
        .unwrap();
    assert!(matches!(
        handshake(client, server, "localhost"),
        Err(rustls::Error::InvalidCertificate(_))
    ));
}

#[test]
fn custom_intermediate_trust_preserves_hostname_validation() {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let root = CertifiedIssuer::self_signed(params.clone(), KeyPair::generate().unwrap()).unwrap();
    let intermediate =
        CertifiedIssuer::signed_by(params, KeyPair::generate().unwrap(), &root).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&leaf_key, &intermediate)
        .unwrap();
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("intermediate.pem");
    std::fs::write(&path, intermediate.pem()).unwrap();
    let config = build_rustls_client_config(Some(&ConfiguredCaBundle {
        source_env: CODEX_CA_CERT_ENV,
        path,
    }))
    .unwrap();
    let server = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![leaf.der().clone()], leaf_key.into())
            .unwrap(),
    );

    assert_eq!(
        handshake(config.clone(), server.clone(), "localhost"),
        Ok(())
    );
    let error = handshake(config, server, "wrong.example").unwrap_err();
    assert!(
        matches!(
            error,
            rustls::Error::InvalidCertificate(
                rustls::CertificateError::NotValidForName
                    | rustls::CertificateError::NotValidForNameContext { .. }
            )
        ),
        "{error:?}"
    );
}

fn handshake(
    config: Arc<rustls::ClientConfig>,
    server: Arc<rustls::ServerConfig>,
    hostname: &'static str,
) -> Result<(), rustls::Error> {
    let mut client = rustls::ClientConnection::new(config, hostname.try_into().unwrap()).unwrap();
    let mut server = rustls::ServerConnection::new(server).unwrap();
    for _ in 0..10 {
        let mut bytes = Vec::new();
        client.write_tls(&mut bytes).unwrap();
        server.read_tls(&mut bytes.as_slice()).unwrap();
        server.process_new_packets()?;
        bytes.clear();
        server.write_tls(&mut bytes).unwrap();
        client.read_tls(&mut bytes.as_slice()).unwrap();
        client.process_new_packets()?;
        if !client.is_handshaking() {
            return Ok(());
        }
    }
    panic!("handshake did not complete");
}
