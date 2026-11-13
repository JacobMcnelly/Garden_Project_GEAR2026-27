-- in:  :ts, :name, :value
-- out: (none) — saves one reading; ignored if that sensor already has one at :ts, or if the name is unknown
INSERT OR IGNORE INTO readings (ts, sensor_id, value)
SELECT :ts, id, :value FROM sensors WHERE name = :name;
