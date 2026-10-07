-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#FPP_FCAS_SUMMARY#FILE01#202605.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS FPP_FCAS_SUMMARY owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#FPP_FCAS_SUMMARY#FILE01#202605010000.CSV
APPEND INTO TABLE FPP_FCAS_SUMMARY
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
RUNNO FLOAT EXTERNAL,
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
CONSTRAINTID CHAR(20),
VERSIONNO FLOAT EXTERNAL,
BIDTYPE CHAR(10),
RELEVANT_REGIONS CHAR(200),
REGULATION_MW FLOAT EXTERNAL,
CONSTRAINT_MARGINAL_VALUE FLOAT EXTERNAL,
P_REGULATION FLOAT EXTERNAL,
BASE_COST FLOAT EXTERNAL,
TSFCAS FLOAT EXTERNAL,
TOTAL_FPP FLOAT EXTERNAL,
RCR FLOAT EXTERNAL,
USAGE_VALUE FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
