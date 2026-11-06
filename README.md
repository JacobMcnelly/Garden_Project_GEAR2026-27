# Garden Monitor

A Raspberry Pi 4 that reads garden sensors every minute, saves the readings, and shows them on a private website with history charts and a **Read now** button for live values.

## Stack

- **Server:** Rust (axum) + SQLite
- **Sensors:** Python
- **Website:** React + coss ui + uPlot
- **Access:** Tailscale (private)

## Status

Planning stage. No code yet; hardware is not chosen.

## Docs

- [ARCHITECTURE.md](ARCHITECTURE.md): how everything fits together, with diagrams.
