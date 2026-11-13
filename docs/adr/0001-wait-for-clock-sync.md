# Wait for clock sync before starting

The Pi 4 has no battery-backed clock, so after a power cut it boots with a stale time until NTP syncs, and any readings stored then would be stamped wrong. `garden.service` waits for `time-sync.target` (with `systemd-time-wait-sync` enabled), so nothing starts — and nothing is recorded — until the clock is right. Trade-off: if the Pi boots with no internet, the app stays down until the network returns; we prefer a gap over wrong timestamps that can't be fixed later.
