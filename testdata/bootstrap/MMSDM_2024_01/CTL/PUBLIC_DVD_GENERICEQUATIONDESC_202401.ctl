-- ************************************************************
-- Title:	PUBLIC_DVD_GENERICEQUATIONDESC_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS GENERICEQUATIONDESC owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_GENERICEQUATIONDESC_202401010000.CSV
APPEND INTO TABLE GENERICEQUATIONDESC
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
EQUATIONID CHAR(20),
DESCRIPTION CHAR(256),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
IMPACT CHAR(64),
SOURCE CHAR(128),
LIMITTYPE CHAR(64),
REASON CHAR(256),
MODIFICATIONS CHAR(256),
ADDITIONALNOTES CHAR(256))
-- End of Script
