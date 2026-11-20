-- in:  :id
-- out: (none) — removes the bed; fails if it still has plantings
DELETE FROM beds WHERE id = :id;
