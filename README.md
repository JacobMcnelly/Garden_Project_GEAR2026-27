# Garden Monitor

A Raspberry Pi 4 in the garden reads the sensors every minute and saves the values. It also runs a private website with history charts and a **Read now** button for live values.

## Status

| Part | State |
|---|---|
| Rust server | Done and tested. Runs on a PC with fake sensors. |
| Database | First version of the tables and queries is done. |
| Sensors | Starter script with fake values. Sensors not chosen yet. |
| Website | Not started. Charts (uPlot) and icons (Lucide) are chosen, the rest is open. |
| Pi install | Scripts written, not yet tried on the Pi. |

## Where to start

Most of us are new to this kind of project, and that's fine. You don't need to understand the whole thing. Read the overview at the top of [ARCHITECTURE.md](ARCHITECTURE.md), then only the section for your part. It lists the commands to run and the few rules that matter. If something is unclear, ask the others in the club instead of guessing, and we'll fix the docs.

Your section:

- Sensors (Python): [For the sensors developer](ARCHITECTURE.md#for-the-sensors-developer)
- Website: [For the website developer](ARCHITECTURE.md#for-the-website-developer)
- Database (SQL): [For the database developer](ARCHITECTURE.md#for-the-database-developer)
- Installing on the Pi: [deploy/setup.md](deploy/setup.md)

[GLOSSARY.md](GLOSSARY.md) defines the words we use. [docs/adr/](docs/adr/) explains decisions that might look odd.
