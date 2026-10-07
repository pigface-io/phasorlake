-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#SSM_ENABLEMENT_COSTS#FILE01#202511.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS SSM_ENABLEMENT_COSTS owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#SSM_ENABLEMENT_COSTS#FILE01#202511010000.CSV
APPEND INTO TABLE SSM_ENABLEMENT_COSTS
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
END_TRADINGDATE DATE "YYYY/MM/DD HH24:MI:SS",
ENABLEMENT_REASON CHAR(40),
ESTIMATED_COSTS FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
