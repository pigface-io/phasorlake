-- ************************************************************
-- Title:	PUBLIC_DVD_SETCPDATAREGION_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS SETCPDATAREGION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_SETCPDATAREGION_202401010000.CSV
APPEND INTO TABLE SETCPDATAREGION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss",
VERSIONNO FLOAT EXTERNAL,
PERIODID FLOAT EXTERNAL,
REGIONID CHAR(10),
SUMIGENERGY FLOAT EXTERNAL,
SUMXGENERGY FLOAT EXTERNAL,
SUMINENERGY FLOAT EXTERNAL,
SUMXNENERGY FLOAT EXTERNAL,
SUMIPOWER FLOAT EXTERNAL,
SUMXPOWER FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
SUMEP FLOAT EXTERNAL)
-- End of Script
