//! Example OxiCloud plugin — ABI v0 (M0 walking skeleton).
//!
//! The default build is the well-behaved "hello" plugin. It exports one handler
//! per event it subscribes to — `on_file_uploaded` and `on_user_login` — each of
//! which reads the event metadata, calls the host `log` function (the only
//! authority a plugin has), and returns `{"ok": true}`.
//!
//! Cargo features select the variants the host's tests load:
//! - `panic` / `sleep` / `net` — make `on_file_uploaded` misbehave (failure
//!   isolation, timeout, network-denial tests);
//! - `wrong_abi` — `abi_version` returns 1 (load-rejection test);
//! - `omit_login` — drops the `on_user_login` export (missing-export test).
//!
//! See `scripts/build-plugin-hello.sh`.

use extism_pdk::*;

/// The one host function OxiCloud exposes, imported from its namespaced module.
#[host_fn("oxicloud:host:v0")]
extern "ExtismHost" {
    fn log(level: String, message: String);
}

/// Required export: which ABI this plugin was built against. The host rejects
/// the plugin at load if this does not equal its own `OXICLOUD_PLUGIN_ABI`.
#[cfg(not(feature = "wrong_abi"))]
#[plugin_fn]
pub fn abi_version() -> FnResult<u32> {
    Ok(0)
}

/// `wrong_abi` variant: claim an ABI the host does not speak.
#[cfg(feature = "wrong_abi")]
#[plugin_fn]
pub fn abi_version() -> FnResult<u32> {
    Ok(1)
}

/// Handler for the `file.uploaded` event.
///
/// `#[plugin_fn]` rewrites the fn signature, so an outer `#[allow]` doesn't
/// reach the inner scope where `input` is bound — hence the `_` prefix on the
/// parameter. The well-behaved tail rebinds it as `input` locally.
#[plugin_fn]
pub fn on_file_uploaded(_input: String) -> FnResult<String> {
    // --- misbehaving variants (compiled in only under their feature) ---------
    #[cfg(feature = "panic")]
    panic!("intentional panic: exercises host failure isolation");

    #[cfg(feature = "sleep")]
    {
        // Busy-loop forever; the host's wall-clock timeout must cancel us.
        let mut spin: u64 = 0;
        loop {
            spin = spin.wrapping_add(1);
            std::hint::black_box(spin);
        }
    }

    // The well-behaved tail is unreachable under the diverging variants above;
    // gate it so the compiler doesn't flag input/tail as unused/dead.
    #[cfg(not(any(feature = "panic", feature = "sleep")))]
    {
        let input = _input;

        #[cfg(feature = "net")]
        {
            // Attempt an outbound HTTP call. The host grants no `allowed_hosts`,
            // so Extism denies this before any socket is opened
            // (offline-deterministic) and the error propagates out.
            let req = HttpRequest::new("https://example.com/");
            let _ = http::request::<()>(&req, None)?;
        }

        let ev: serde_json::Value = serde_json::from_str(&input)?;
        let path = ev["payload"]["path"].as_str().unwrap_or("<unknown>");
        let size = ev["payload"]["size"].as_u64().unwrap_or(0);

        unsafe {
            log(
                "info".to_string(),
                format!("hello plugin saw upload: {path} ({size} bytes)"),
            )?;
        }
        Ok(serde_json::json!({ "ok": true }).to_string())
    }
}

/// Handler for the `user.login` event. Dropped by the `omit_login` variant so
/// the host's missing-export validation has something to reject.
#[cfg(not(feature = "omit_login"))]
#[plugin_fn]
pub fn on_user_login(input: String) -> FnResult<String> {
    let ev: serde_json::Value = serde_json::from_str(&input)?;
    let user_id = ev["payload"]["user_id"].as_str().unwrap_or("<unknown>");
    let first_login = ev["payload"]["first_login"].as_bool().unwrap_or(false);

    unsafe {
        log(
            "info".to_string(),
            format!("hello plugin saw login: user {user_id} (first_login={first_login})"),
        )?;
    }
    Ok(serde_json::json!({ "ok": true }).to_string())
}
