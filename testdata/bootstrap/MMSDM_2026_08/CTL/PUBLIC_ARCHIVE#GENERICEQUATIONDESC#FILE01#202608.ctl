-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#GENERICEQUATIONDESC#FILE01#202608.ctl
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
INFILE PUBLIC_ARCHIVE#GENERICEQUATIONDESC#FILE01#202608010000.CSV
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
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
IMPACT CHAR(64),
SOURCE CHAR(128),
LIMITTYPE CHAR(64),
REASON CHAR(256),
MODIFICATIONS CHAR(256),
ADDITIONALNOTES CHAR(256))
-- End of Script
