-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#FPP_HIST_PERFORMANCE#FILE01#202605.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS FPP_HIST_PERFORMANCE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#FPP_HIST_PERFORMANCE#FILE01#202605010000.CSV
APPEND INTO TABLE FPP_HIST_PERFORMANCE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
FPP_UNITID CHAR(20),
EFFECTIVE_START_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
EFFECTIVE_END_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
VERSIONNO FLOAT EXTERNAL,
HIST_PERIOD_START_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
HIST_PERIOD_END_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
REG_HIST_RAISE_PERFORMANCE FLOAT EXTERNAL,
REG_HIST_LOWER_PERFORMANCE FLOAT EXTERNAL,
FPP_HIST_RAISE_PERFORMANCE FLOAT EXTERNAL,
FPP_HIST_LOWER_PERFORMANCE FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
