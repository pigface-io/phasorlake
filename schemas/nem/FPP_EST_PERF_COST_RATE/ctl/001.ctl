-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#FPP_EST_PERF_COST_RATE#FILE01#202412.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS FPP_EST_PERF_COST_RATE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#FPP_EST_PERF_COST_RATE#FILE01#202412010000.CSV
APPEND INTO TABLE FPP_EST_PERF_COST_RATE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
CONSTRAINTID CHAR(20),
VERSIONNO FLOAT EXTERNAL,
BIDTYPE CHAR(10),
RELEVANT_REGIONS CHAR(200),
FPP_PAYMENT_RATE FLOAT EXTERNAL,
FPP_RECOVERY_RATE FLOAT EXTERNAL,
USED_FCAS_RATE FLOAT EXTERNAL,
UNUSED_FCAS_RATE FLOAT EXTERNAL)
-- End of Script
