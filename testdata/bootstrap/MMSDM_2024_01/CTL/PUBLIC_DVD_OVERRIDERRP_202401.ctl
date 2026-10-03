-- ************************************************************
-- Title:	PUBLIC_DVD_OVERRIDERRP_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS OVERRIDERRP owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_OVERRIDERRP_202401010000.CSV
APPEND INTO TABLE OVERRIDERRP
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
REGIONID CHAR(10),
STARTDATE DATE "yyyy/mm/dd hh24:mi:ss",
STARTPERIOD FLOAT EXTERNAL,
ENDDATE DATE "yyyy/mm/dd hh24:mi:ss",
ENDPERIOD FLOAT EXTERNAL,
RRP FLOAT EXTERNAL,
DESCRIPTION CHAR(128),
AUTHORISESTART CHAR(15),
AUTHORISEEND CHAR(15),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
