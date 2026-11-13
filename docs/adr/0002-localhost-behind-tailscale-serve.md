# Bind to localhost, expose only via `tailscale serve`

The app has no login, and pump control is planned. The server binds `127.0.0.1:8080` and is reached only through `tailscale serve`, which also gives HTTPS for free. Binding `0.0.0.0` would expose it to anyone on the home Wi-Fi. This is the app's only access control: do not change the bind address without adding auth.
