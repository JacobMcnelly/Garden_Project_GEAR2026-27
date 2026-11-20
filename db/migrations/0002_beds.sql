CREATE TABLE beds (
  id       INTEGER PRIMARY KEY,
  name     TEXT NOT NULL UNIQUE,      -- "Bed 3"
  location TEXT NOT NULL DEFAULT '',  -- "north fence, second from the gate"
  notes    TEXT NOT NULL DEFAULT ''
) STRICT;

CREATE TABLE plantings (
  id         INTEGER PRIMARY KEY,
  bed_id     INTEGER NOT NULL REFERENCES beds(id),
  plant      TEXT NOT NULL,             -- "Tomato"
  variety    TEXT NOT NULL DEFAULT '',  -- "Roma"
  planted_on TEXT NOT NULL,             -- "2026-04-15"
  removed_on TEXT,                      -- empty while still growing
  notes      TEXT NOT NULL DEFAULT '',
  changed_by TEXT NOT NULL,             -- Tailscale login of the last admin to edit it
  changed_at INTEGER NOT NULL           -- unix seconds
) STRICT;
