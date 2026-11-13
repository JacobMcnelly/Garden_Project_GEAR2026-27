-- in:  :from, :to, :bucket (unix seconds)
-- out: name, unit, t, avg_value  (one row per sensor per bucket that has data, ordered by name, t)
SELECT s.name, s.unit, r.ts / :bucket * :bucket AS t, AVG(r.value) AS avg_value
FROM readings r JOIN sensors s ON s.id = r.sensor_id
WHERE r.ts BETWEEN :from AND :to
GROUP BY r.sensor_id, t
ORDER BY s.name, t;
