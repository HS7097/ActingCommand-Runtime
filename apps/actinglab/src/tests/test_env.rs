// SPDX-License-Identifier: AGPL-3.0-only

use super::{TRUSTED_REMOTE_CLIENT_CERT_ENV, TRUSTED_REMOTE_TOKEN_ENV};
use std::env;
use std::ffi::OsString;

pub(super) struct TrustedRemoteEnvGuard {
    previous_token: Option<OsString>,
    previous_client_certificate: Option<OsString>,
}

impl TrustedRemoteEnvGuard {
    pub(super) fn clear() -> Self {
        let guard = Self {
            previous_token: env::var_os(TRUSTED_REMOTE_TOKEN_ENV),
            previous_client_certificate: env::var_os(TRUSTED_REMOTE_CLIENT_CERT_ENV),
        };
        unsafe {
            env::remove_var(TRUSTED_REMOTE_TOKEN_ENV);
            env::remove_var(TRUSTED_REMOTE_CLIENT_CERT_ENV);
        }
        guard
    }

    pub(super) fn with_token(token: &str) -> Self {
        let guard = Self::clear();
        unsafe {
            env::set_var(TRUSTED_REMOTE_TOKEN_ENV, token);
        }
        guard
    }
}

impl Drop for TrustedRemoteEnvGuard {
    fn drop(&mut self) {
        restore_env_var(TRUSTED_REMOTE_TOKEN_ENV, self.previous_token.take());
        restore_env_var(
            TRUSTED_REMOTE_CLIENT_CERT_ENV,
            self.previous_client_certificate.take(),
        );
    }
}

fn restore_env_var(name: &str, previous: Option<OsString>) {
    unsafe {
        match previous {
            Some(value) => env::set_var(name, value),
            None => env::remove_var(name),
        }
    }
}

#[test]
fn restores_previous_values() {
    let _guard = super::env_lock();
    let _original_env = TrustedRemoteEnvGuard::clear();
    unsafe {
        env::set_var(TRUSTED_REMOTE_TOKEN_ENV, "previous-token");
        env::set_var(
            TRUSTED_REMOTE_CLIENT_CERT_ENV,
            "previous-client-certificate",
        );
    }

    {
        let _cleared_env = TrustedRemoteEnvGuard::clear();
        assert_eq!(env::var_os(TRUSTED_REMOTE_TOKEN_ENV), None);
        assert_eq!(env::var_os(TRUSTED_REMOTE_CLIENT_CERT_ENV), None);
    }

    assert_eq!(
        env::var_os(TRUSTED_REMOTE_TOKEN_ENV),
        Some(OsString::from("previous-token"))
    );
    assert_eq!(
        env::var_os(TRUSTED_REMOTE_CLIENT_CERT_ENV),
        Some(OsString::from("previous-client-certificate"))
    );
}
