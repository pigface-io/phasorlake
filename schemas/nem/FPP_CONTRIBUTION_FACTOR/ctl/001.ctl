-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#FPP_CONTRIBUTION_FACTOR#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS FPP_CONTRIBUTION_FACTOR owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#FPP_CONTRIBUTION_FACTOR#FILE01#202608010000.CSV
APPEND INTO TABLE FPP_CONTRIBUTION_FACTOR
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
FPP_UNITID CHAR(20),
VERSIONNO FLOAT EXTERNAL,
BIDTYPE CHAR(10),
CONTRIBUTION_FACTOR FLOAT EXTERNAL,
NEGATIVE_CONTRIBUTION_FACTOR FLOAT EXTERNAL,
DEFAULT_CONTRIBUTION_FACTOR FLOAT EXTERNAL,
CF_REASON_FLAG FLOAT EXTERNAL,
CF_ABS_POSITIVE_PERF_TOTAL FLOAT EXTERNAL,
CF_ABS_NEGATIVE_PERF_TOTAL FLOAT EXTERNAL,
NCF_ABS_NEGATIVE_PERF_TOTAL FLOAT EXTERNAL,
PARTICIPANTID CHAR(20),
SETTLEMENTS_UNITID CHAR(50),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
