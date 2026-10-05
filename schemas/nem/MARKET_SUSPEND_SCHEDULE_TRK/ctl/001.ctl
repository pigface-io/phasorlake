-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#MARKET_SUSPEND_SCHEDULE_TRK#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MARKET_SUSPEND_SCHEDULE_TRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#MARKET_SUSPEND_SCHEDULE_TRK#FILE01#202608010000.CSV
APPEND INTO TABLE MARKET_SUSPEND_SCHEDULE_TRK
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
EFFECTIVEDATE DATE "YYYY/MM/DD HH24:MI:SS",
SOURCE_START_DATE DATE "YYYY/MM/DD HH24:MI:SS",
SOURCE_END_DATE DATE "YYYY/MM/DD HH24:MI:SS",
COMMENTS CHAR(1000),
AUTHORISEDDATE DATE "YYYY/MM/DD HH24:MI:SS",
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
