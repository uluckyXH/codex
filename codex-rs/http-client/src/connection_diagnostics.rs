//! Connection failure details that never print arbitrary error strings or request data.

use std::error::Error;

pub(crate) fn diagnostic_suffix(error: &(dyn Error + 'static)) -> String {
    let mut source = Some(error);
    let mut details = Vec::new();
    // Bound traversal even for a third-party error with a cyclic source chain.
    for _ in 0..32 {
        let Some(error) = source else { break };
        if let Some(error) = error.downcast_ref::<std::io::Error>() {
            let detail = match error.raw_os_error() {
                Some(code) => format!("I/O {:?}, os error {code}", error.kind()),
                None => format!("I/O {:?}", error.kind()),
            };
            if !details.contains(&detail) {
                details.push(detail);
            }
        }
        let tls = if let Some(error) = error.downcast_ref::<rustls::Error>() {
            Some(match error {
                rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer) => {
                    "TLS certificate: unknown issuer"
                }
                rustls::Error::InvalidCertificate(_) => "TLS certificate verification failed",
                _ => "TLS handshake failed",
            })
        } else if error.is::<native_tls::Error>() {
            Some("native TLS handshake failed")
        } else {
            None
        };
        if let Some(tls) = tls
            && !details.iter().any(|detail| detail == tls)
        {
            details.push(tls.to_string());
        }
        // io::Error's source() may skip the wrapped error itself.
        source = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .map(|inner| inner as &(dyn Error + 'static))
            .or_else(|| error.source());
    }
    if details.is_empty() {
        String::new()
    } else {
        format!(" ({})", details.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::diagnostic_suffix;

    #[test]
    fn reports_connection_errno_without_arbitrary_message() {
        let os = std::io::Error::from_raw_os_error(libc_error_connection_refused());
        let message = diagnostic_suffix(&os);
        assert!(message.contains("ConnectionRefused"));
        assert!(message.contains("os error"));

        let private = std::io::Error::other("https://user:secret@example.test/?token=hidden");
        let message = diagnostic_suffix(&private);
        assert!(message.contains("Other"));
        assert!(!message.contains("secret"));
        assert!(!message.contains("hidden"));
        assert!(!message.contains("example.test"));
    }

    fn libc_error_connection_refused() -> i32 {
        if cfg!(target_os = "macos") {
            61
        } else if cfg!(windows) {
            10061
        } else {
            111
        }
    }

    #[test]
    fn reports_nested_certificate_failure_without_certificate_contents() {
        let error = std::io::Error::other(rustls::Error::InvalidCertificate(
            rustls::CertificateError::UnknownIssuer,
        ));
        let message = diagnostic_suffix(&error);
        assert!(message.contains("unknown issuer"));
    }
}
