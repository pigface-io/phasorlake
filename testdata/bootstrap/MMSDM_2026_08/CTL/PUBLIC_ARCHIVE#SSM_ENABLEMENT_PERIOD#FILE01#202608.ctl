-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#SSM_ENABLEMENT_PERIOD#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS SSM_ENABLEMENT_PERIOD owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#SSM_ENABLEMENT_PERIOD#FILE01#202608010000.CSV
APPEND INTO TABLE SSM_ENABLEMENT_PERIOD
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
INSTRUCTION_ID CHAR(20),
DUID CHAR(20),
ENABLEMENT_START_INTERVAL DATE "YYYY/MM/DD HH24:MI:SS",
ENABLEMENT_END_INTERVAL DATE "YYYY/MM/DD HH24:MI:SS",
ENABLEMENT_REASON CHAR(40),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
