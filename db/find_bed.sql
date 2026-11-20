-- in:  :id
-- out: id, name, location, notes  (no row if the bed doesn't exist)
SELECT id, name, location, notes FROM beds WHERE id = :id;
