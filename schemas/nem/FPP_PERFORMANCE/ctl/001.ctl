-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#FPP_PERFORMANCE#FILE01#202412.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS FPP_PERFORMANCE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#FPP_PERFORMANCE#FILE01#202412010000.CSV
APPEND INTO TABLE FPP_PERFORMANCE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
FPP_UNITID CHAR(20),
VERSIONNO FLOAT EXTERNAL,
RAISE_PERFORMANCE FLOAT EXTERNAL,
RAISE_REASON_FLAG FLOAT EXTERNAL,
LOWER_PERFORMANCE FLOAT EXTERNAL,
LOWER_REASON_FLAG FLOAT EXTERNAL,
PARTICIPANTID CHAR(20))
-- End of Script
