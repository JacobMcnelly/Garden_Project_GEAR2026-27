-- in:  :current (1 = only plantings still growing, 0 = all)
-- out: id, bed_id, plant, variety, planted_on, removed_on, notes, changed_by, changed_at  (ordered by planted_on, id)
SELECT id, bed_id, plant, variety, planted_on, removed_on, notes, changed_by, changed_at
FROM plantings
WHERE :current = 0 OR removed_on IS NULL
ORDER BY planted_on, id;
