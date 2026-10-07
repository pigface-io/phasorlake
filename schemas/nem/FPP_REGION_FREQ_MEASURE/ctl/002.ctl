-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#FPP_REGION_FREQ_MEASURE#FILE01#202605.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS FPP_REGION_FREQ_MEASURE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#FPP_REGION_FREQ_MEASURE#FILE01#202605010000.CSV
APPEND INTO TABLE FPP_REGION_FREQ_MEASURE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
MEASUREMENT_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
REGIONID CHAR(20),
VERSIONNO FLOAT EXTERNAL,
FREQ_DEVIATION_HZ FLOAT EXTERNAL,
HZ_QUALITY_FLAG FLOAT EXTERNAL,
FREQ_MEASURE_HZ FLOAT EXTERNAL,
FM_ALIGNMENT_FLAG FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
