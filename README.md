# Garden Monitor

A Raspberry Pi 4 in the garden reads the sensors every minute and saves the values. It also runs a private website with history charts and a **Read now** button for live values.

## Status

| Part | State |
|---|---|
| Rust server | Done and tested. Runs on a PC with fake sensors. |
| Database | First version of the tables and queries is done. |
| Sensors | Starter script with fake values. Sensors not chosen yet. |
| Website | Not started. |
| Pi install | Scripts written, not yet tried on the Pi. |

## Where to start

Read the overview at the top of [ARCHITECTURE.md](ARCHITECTURE.md), then the section for your part:

- Sensors (Python): [For the sensors developer](ARCHITECTURE.md#for-the-sensors-developer)
- Website (React): [For the website developer](ARCHITECTURE.md#for-the-website-developer)
- Database (SQL): [For the database developer](ARCHITECTURE.md#for-the-database-developer)
- Installing on the Pi: [deploy/setup.md](deploy/setup.md)

[GLOSSARY.md](GLOSSARY.md) defines the words we use. [docs/adr/](docs/adr/) explains decisions that might look odd.
