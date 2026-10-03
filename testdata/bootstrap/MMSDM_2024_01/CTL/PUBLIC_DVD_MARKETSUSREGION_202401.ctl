-- ************************************************************
-- Title:	PUBLIC_DVD_MARKETSUSREGION_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MARKETSUSREGION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_MARKETSUSREGION_202401010000.CSV
APPEND INTO TABLE MARKETSUSREGION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SUSPENSIONID CHAR(10),
REGIONID CHAR(10),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
