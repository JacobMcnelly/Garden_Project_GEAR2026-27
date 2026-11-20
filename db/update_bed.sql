-- in:  :id, :name, :location, :notes
-- out: id, name, location, notes  (the changed bed); fails if the name is taken
UPDATE beds SET name = :name, location = :location, notes = :notes WHERE id = :id
RETURNING id, name, location, notes;
