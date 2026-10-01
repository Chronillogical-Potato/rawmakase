-- Reports received per UTC day, up to one past the budget, so the daily
-- budget is reserved in the same transaction that counts a report. Rows older than
-- yesterday are deleted by the daily sweep.
CREATE TABLE report_budget (
  day TEXT NOT NULL PRIMARY KEY,
  used INTEGER NOT NULL
) WITHOUT ROWID;
