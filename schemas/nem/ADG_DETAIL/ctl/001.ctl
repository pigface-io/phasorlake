-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#ADG_DETAIL#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS ADG_DETAIL owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#ADG_DETAIL#FILE01#202608010000.CSV
APPEND INTO TABLE ADG_DETAIL
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
ADG_ID CHAR(20),
EFFECTIVEDATE DATE "YYYY/MM/DD HH24:MI:SS",
VERSION_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
ADG_TYPE CHAR(20),
AUTHORISEDDATE DATE "YYYY/MM/DD HH24:MI:SS",
AUTHORISEDBY CHAR(15),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
