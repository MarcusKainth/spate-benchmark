-- Removes everything arm.sql creates. Run by the harness before every
-- repetition's TRUNCATE of the shared target, so the materialized view is never
-- live while sensor_events is truncated, and again so no arm object outlives
-- the arm. View first: dropping the landing table out from under a live view
-- would leave a view over nothing.
DROP VIEW IF EXISTS sensor_batches_mv;
DROP TABLE IF EXISTS sensor_batches_landing;
