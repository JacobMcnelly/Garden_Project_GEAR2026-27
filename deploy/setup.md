# Pi setup

How to install the garden monitor on a fresh Raspberry Pi 4. Do it once. To update later, see [Updating](#updating).

In the commands below, replace `<pi>` with your Pi's name on Tailscale (or its address), and `pi` with your login name on the Pi.

## On your PC: build

From the project folder:

```sh
cd server && cargo zigbuild --release --target aarch64-unknown-linux-gnu && cd ..
cd web && npm run build && cd ..
```

Then copy everything to the Pi (into a folder called `upload` in your home folder):

```sh
ssh pi@<pi> mkdir -p upload/web
scp server/target/aarch64-unknown-linux-gnu/release/garden pi@<pi>:upload/
scp -r web/dist pi@<pi>:upload/web/
scp -r sensors deploy pi@<pi>:upload/
```

If you have a `sensors/.venv` folder on your PC, delete it from `upload/sensors/` on the Pi. It only works on your PC.

## On the Pi: install

Log in with `ssh pi@<pi>`, then:

1. Use 64-bit Raspberry Pi OS. Install the tools:
   ```sh
   sudo apt update && sudo apt install -y sqlite3
   curl -LsSf https://astral.sh/uv/install.sh | sudo env UV_INSTALL_DIR=/usr/local/bin sh
   ```
   If Tailscale isn't installed yet: `curl -fsSL https://tailscale.com/install.sh | sh`, then `sudo tailscale up`.

2. Create the `garden` user that runs the app. The `gpio` and `i2c` groups let it use the sensors:
   ```sh
   sudo useradd --system --home-dir /opt/garden --shell /usr/sbin/nologin garden
   sudo usermod -aG gpio,i2c garden
   ```

3. Put the files in `/opt/garden`:
   ```sh
   sudo mkdir -p /opt/garden
   sudo cp -r ~/upload/garden ~/upload/web ~/upload/sensors /opt/garden/
   sudo chmod +x /opt/garden/garden
   sudo chown -R garden:garden /opt/garden
   ```

4. Set up Python for the sensors (needs `sensors/pyproject.toml`, which the sensors developer adds):
   ```sh
   cd /opt/garden/sensors && sudo -u garden -H uv sync && cd
   ```

5. Make the app wait for the correct time after a power cut ([ADR 0001](../docs/adr/0001-wait-for-clock-sync.md)):
   ```sh
   sudo systemctl enable systemd-time-wait-sync
   ```

6. Install and start the app and the nightly backup:
   ```sh
   sudo cp ~/upload/deploy/garden.service ~/upload/deploy/garden-backup.service ~/upload/deploy/garden-backup.timer /etc/systemd/system/
   sudo systemctl daemon-reload
   sudo systemctl enable --now garden.service garden-backup.timer
   ```

7. Make the website reachable over Tailscale. The app only listens on the Pi itself ([ADR 0002](../docs/adr/0002-localhost-behind-tailscale-serve.md)):
   ```sh
   sudo tailscale serve --bg 8080
   ```
   It prints the address to open, like `https://<pi>.<your-tailnet>.ts.net`.

## Check it's running

```sh
systemctl status garden            # should say "active (running)"
journalctl -u garden -f            # live log; Ctrl-C to stop watching
systemctl list-timers garden-backup
sudo systemctl start garden-backup && ls /opt/garden/backups   # test a backup now
```

## Updating

Build and copy to `~/upload` as above (only the parts that changed), then on the Pi:

- **Program:**
  ```sh
  sudo install -o garden -g garden -m 755 ~/upload/garden /opt/garden/garden
  sudo systemctl restart garden
  ```
  (`install` instead of `cp`, because `cp` can't overwrite a program that is running.)
- **Website:** `sudo rm -r /opt/garden/web/dist && sudo cp -r ~/upload/web/dist /opt/garden/web/ && sudo chown -R garden:garden /opt/garden/web`. No restart needed.
- **Sensors:** `sudo cp -r ~/upload/sensors /opt/garden/ && sudo chown -R garden:garden /opt/garden/sensors`, then `cd /opt/garden/sensors && sudo -u garden -H uv sync && cd`, then `sudo systemctl restart garden`.
- **Service files:** copy them to `/etc/systemd/system/` again, then `sudo systemctl daemon-reload && sudo systemctl restart garden`.
