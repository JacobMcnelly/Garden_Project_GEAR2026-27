# Listen on localhost, reach it only through `tailscale serve`

The server listens only on `127.0.0.1:8080`. People reach it through `tailscale serve`, which also gives us HTTPS for free. Listening on `0.0.0.0` would let anyone on the same Wi-Fi open it.

This is the app's only access control. Admins are recognised by a header that `tailscale serve` adds ([ADR 0003](0003-admins-by-tailscale-identity.md)). If the server were reachable directly, anyone could send that header and act as an admin, and later run the pump. So never change the address it listens on without adding a real login first.
