# Pi setup

How to install the garden monitor on a Raspberry Pi 4, and how to update it. One script does both.

In the commands below, replace `<pi>` with your Pi's name on Tailscale (or its address), and `pi` with your login name on the Pi.

## Once: prepare the Pi

1. Use 64-bit Raspberry Pi OS, with SSH turned on.
2. Install Tailscale on it and log in:
   ```sh
   curl -fsSL https://tailscale.com/install.sh | sh
   sudo tailscale up
   ```
3. Check that `ssh pi@<pi>` works from your PC.

On your PC you need the build tools listed in [Install or update the Pi](../ARCHITECTURE.md#install-or-update-the-pi), plus bun once the website exists. On Windows, run the commands below in Git Bash.

## Install or update

From the project folder on your PC (Git Bash on Windows):

```sh
deploy/deploy.sh pi@<pi>
```

It builds everything, copies it to the Pi, installs it in `/opt/garden`, restarts the app and checks that it answers. The first time, it also sets up the Pi: the `garden` user, `sqlite3`, `uv`, waiting for the clock ([ADR 0001](../docs/adr/0001-wait-for-clock-sync.md)), the nightly backup and `tailscale serve` ([ADR 0002](../docs/adr/0002-localhost-behind-tailscale-serve.md)).

To update only some parts, name them:

```sh
deploy/deploy.sh pi@<pi> server     # the Rust program
deploy/deploy.sh pi@<pi> web        # the website
deploy/deploy.sh pi@<pi> sensors    # the Python code (runs uv sync)
deploy/deploy.sh pi@<pi> units      # the files in deploy/ (service, backup timer)
```

The website is then at `https://<pi>.<your-tailnet>.ts.net`.

If your Pi login needs a password for `sudo`, the script asks for it. If the final check fails, read the log on the Pi with `journalctl -u garden -n 50`.

## Check it's running

On the Pi (`ssh pi@<pi>`):

```sh
systemctl status garden            # should say "active (running)"
journalctl -u garden -f            # live log; Ctrl-C to stop watching
systemctl list-timers garden-backup
sudo systemctl start garden-backup && ls /opt/garden/backups   # test a backup now
```
