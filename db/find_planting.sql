-- in:  :id
-- out: id, bed_id, plant, variety, planted_on, removed_on, notes, changed_by, changed_at  (no row if it doesn't exist)
SELECT id, bed_id, plant, variety, planted_on, removed_on, notes, changed_by, changed_at
FROM plantings WHERE id = :id;
