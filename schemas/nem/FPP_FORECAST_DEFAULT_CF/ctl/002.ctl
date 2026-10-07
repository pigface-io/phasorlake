-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#FPP_FORECAST_DEFAULT_CF#FILE01#202504.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS FPP_FORECAST_DEFAULT_CF owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#FPP_FORECAST_DEFAULT_CF#FILE01#202504010000.CSV
APPEND INTO TABLE FPP_FORECAST_DEFAULT_CF
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
FPP_UNITID CHAR(20),
CONSTRAINTID CHAR(20),
EFFECTIVE_START_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
EFFECTIVE_END_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
VERSIONNO FLOAT EXTERNAL,
BIDTYPE CHAR(10),
REGIONID CHAR(20),
DEFAULT_CONTRIBUTION_FACTOR FLOAT EXTERNAL,
DCF_REASON_FLAG FLOAT EXTERNAL,
DCF_ABS_NEGATIVE_PERF_TOTAL FLOAT EXTERNAL,
SETTLEMENTS_UNITID CHAR(50))
-- End of Script
