-- ************************************************************
-- Title:	PUBLIC_DVD_MTPASA_RESERVELIMIT_REGION_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MTPASA_RESERVELIMIT_REGION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_MTPASA_RESERVELIMIT_REGION_202401010000.CSV
APPEND INTO TABLE MTPASA_RESERVELIMIT_REGION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
VERSION_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
RESERVELIMITID CHAR(20),
REGIONID CHAR(20),
COEF FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
