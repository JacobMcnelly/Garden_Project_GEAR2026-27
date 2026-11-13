-- in:  :name, :unit
-- out: (none) — adds the sensor, or updates its unit; its id never changes
INSERT INTO sensors (name, unit) VALUES (:name, :unit)
ON CONFLICT (name) DO UPDATE SET unit = excluded.unit;
