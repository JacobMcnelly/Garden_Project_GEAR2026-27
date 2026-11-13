# Listen on localhost, reach it only through `tailscale serve`

The app has no login, and pump control is planned. So the server listens only on `127.0.0.1:8080`, and people reach it through `tailscale serve`, which also gives us HTTPS for free. Listening on `0.0.0.0` would let anyone on the same Wi-Fi open it.

This is the app's only access control. Don't change the address it listens on without adding a login first.
