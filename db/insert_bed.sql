-- in:  :name, :location, :notes
-- out: id, name, location, notes  (the new bed); fails if the name is taken
INSERT INTO beds (name, location, notes) VALUES (:name, :location, :notes)
RETURNING id, name, location, notes;
