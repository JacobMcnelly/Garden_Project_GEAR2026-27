# Wait for clock sync before starting

The Pi 4 has no clock battery. After a power cut it boots with the wrong time until it reaches an internet time server, and any readings saved before then would get wrong timestamps.

So `garden.service` waits for `time-sync.target`, with `systemd-time-wait-sync` enabled. Nothing starts, and nothing is saved, until the clock is right.

The cost: if the Pi boots without internet, the app stays down until the network comes back. We prefer a gap in the data to timestamps that are wrong and can't be fixed later.
