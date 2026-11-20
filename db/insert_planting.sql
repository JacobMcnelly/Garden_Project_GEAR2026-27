-- in:  :bed_id, :plant, :variety, :planted_on, :removed_on, :notes, :changed_by, :changed_at
-- out: id, bed_id, plant, variety, planted_on, removed_on, notes, changed_by, changed_at  (the new planting); fails if the bed doesn't exist
INSERT INTO plantings (bed_id, plant, variety, planted_on, removed_on, notes, changed_by, changed_at)
VALUES (:bed_id, :plant, :variety, :planted_on, :removed_on, :notes, :changed_by, :changed_at)
RETURNING id, bed_id, plant, variety, planted_on, removed_on, notes, changed_by, changed_at;
