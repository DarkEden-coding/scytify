# Librespot OAuth 0.8.0 async callback patch

Source: the crates.io librespot-oauth 0.8.0 release, MIT license included.
The root Cargo.toml patches that exact version, including librespot-core's copy.
The default TLS feature is set to rustls native roots to match this application. The workspace resolver also records upstream optional
native-tls packages in Cargo.lock; they are not compiled in the app. Existing
package versions remain pinned.

Upstream `get_access_token_async` calls the synchronous `get_authcode_listener`.
Wrapping it in Tokio timeout cannot interrupt its blocking accept/read. This copy
changes only async authorization callback handling and removes secret-bearing
trace logs. The async flow binds a Tokio listener before opening the browser,
checks callback path and state, handles consent denial/browser-open failure,
bounds request size and read/write duration, and drops its sockets on cancellation.
The caller supplies the overall authorization timeout. Sync OAuth and refresh APIs
are retained; this app uses its existing bounded public-client refresh request so
rotated refresh tokens are retained when Spotify omits a replacement.

The async method now requires a loopback-style HTTP redirect with an explicit port
rather than falling back to blocking stdin. This app uses librespot v0.8.0 main.rs's
default `http://127.0.0.1:5588/login`. The client ID is from librespot-core 0.8.0
config.rs KEYMASTER. The app's five streaming/Connect scopes are a subset of
main.rs OAUTH_SCOPES. Original CLI source inspected at:
https://github.com/librespot-org/librespot/blob/v0.8.0/src/main.rs

Run the local callback tests from the project root:

```sh
cargo test --locked -p librespot-oauth --no-default-features --features rustls-tls-native-roots
```
