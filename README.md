# Garden Monitor

Code from the SJU GEAR Club 2026-2027 team, for the SJU Gardening Club's garden sensor project.

A Raspberry Pi 4 in the garden reads the sensors every minute and saves the values. It also runs a private website: a card per sensor with its latest value, a history chart for each one, and a list of the garden's beds and what grows in them, kept up to date by gardening admins.

## Status

| Part | State |
|---|---|
| Rust server | Done and tested. Runs on a PC with fake sensors. |
| Database | First version of the tables and queries is done. |
| Sensors | Starter script with fake values. Sensors not chosen yet. |
| Website | Not started. Charts (uPlot) and icons (Lucide) are chosen, the rest is open. |
| Beds and plantings | Built in the server. Website pages not started. See [Beds and plantings](ARCHITECTURE.md#beds-and-plantings). |
| Pi install | Scripts written, not yet tried on the Pi. |

## Where to start

Most of us are new to this kind of project, and that's fine. You don't need to understand the whole thing. Read the overview at the top of [ARCHITECTURE.md](ARCHITECTURE.md), then only the section for your part. It lists the commands to run and the few rules that matter. If something is unclear, ask the others in the club instead of guessing, and we'll fix the docs.

Your section:

- Sensors (Python): [For the sensors developer](ARCHITECTURE.md#for-the-sensors-developer)
- Website: [For the website developer](ARCHITECTURE.md#for-the-website-developer)
- Database (SQL): [For the database developer](ARCHITECTURE.md#for-the-database-developer)
- Installing on the Pi: [deploy/setup.md](deploy/setup.md)

[GLOSSARY.md](GLOSSARY.md) defines the words we use. [docs/adr/](docs/adr/) explains decisions that might look odd.
