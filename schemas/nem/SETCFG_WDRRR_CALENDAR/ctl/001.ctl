-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#SETCFG_WDRRR_CALENDAR#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS SETCFG_WDRRR_CALENDAR owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#SETCFG_WDRRR_CALENDAR#FILE01#202408010000.CSV
APPEND INTO TABLE SETCFG_WDRRR_CALENDAR
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
WDRRRPERIOD CHAR(20),
REGIONID CHAR(20),
VERSION_DATETIME TIMESTAMP(3),
STARTDATE DATE "YYYY/MM/DD HH24:MI:SS",
ENDDATE DATE "YYYY/MM/DD HH24:MI:SS",
LASTCHANGED TIMESTAMP(3))
-- End of Script
