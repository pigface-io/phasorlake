-- The month of the source an ingest read from, as `YYYY-MM`, where it is
-- known. A table's hand-written rules can be bounded by months, so the
-- month is part of what decides how a block is read, and is needed to
-- tell, without opening the source, whether its rules still hold.
ALTER TABLE ingests ADD COLUMN source_period TEXT;
