-- in:  :id, :bed_id, :plant, :variety, :planted_on, :removed_on, :notes, :changed_by, :changed_at
-- out: id, bed_id, plant, variety, planted_on, removed_on, notes, changed_by, changed_at  (the changed planting); fails if the bed doesn't exist
UPDATE plantings SET bed_id = :bed_id, plant = :plant, variety = :variety, planted_on = :planted_on,
  removed_on = :removed_on, notes = :notes, changed_by = :changed_by, changed_at = :changed_at
WHERE id = :id
RETURNING id, bed_id, plant, variety, planted_on, removed_on, notes, changed_by, changed_at;
