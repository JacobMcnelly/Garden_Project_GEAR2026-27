# Admins are recognised by their Tailscale login, not a password

Gardening admins add and edit beds and plantings. Everyone else only views. We don't write a login system. `tailscale serve` already adds a `Tailscale-User-Login` header to every request from a Tailscale user, and removes that header if a client tries to send its own. Rust compares that login with the admin list in `GARDEN_ADMINS`, a setting kept in `/etc/garden.env` on the Pi and never in git, because the repo is public. A request without the header, or from someone not on the list, is a viewer.

Rules that keep this safe:
- Every API route that changes data checks the admin list. Pages hiding their edit buttons is only for convenience.
- The server keeps listening only on `127.0.0.1` ([ADR 0002](0002-localhost-behind-tailscale-serve.md)). If it were reachable directly, anyone could fake the header.
- Routes that change data accept only JSON bodies and send no CORS headers. A web page on another site then can't make an admin's browser change data behind their back.

How it grows without code changes:
- More viewers than the free plan's 6 users: share the Pi with them from the Tailscale admin console. They keep their own Tailscale account, and Rust still sees their login.
- Public viewing: turn on Tailscale Funnel. Funnel visitors arrive without the header, so they are viewers automatically. This needs its own ADR first, because the sensor data becomes public too.
- More roles than admin and viewer: move the list from `GARDEN_ADMINS` into a database table.

Trade-off: an admin must have Tailscale on the device they edit from. Sign-in then happens once, in the Tailscale app, and there are no passwords for us to store or leak.
