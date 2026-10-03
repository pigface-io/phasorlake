-- ************************************************************
-- Title:	PUBLIC_DVD_EMSMASTER_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS EMSMASTER owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_EMSMASTER_202401010000.CSV
APPEND INTO TABLE EMSMASTER
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SPD_ID CHAR(21),
SPD_TYPE CHAR(1),
DESCRIPTION CHAR(255),
GROUPING_ID CHAR(20),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
