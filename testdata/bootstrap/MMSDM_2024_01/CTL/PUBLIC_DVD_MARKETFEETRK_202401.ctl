-- ************************************************************
-- Title:	PUBLIC_DVD_MARKETFEETRK_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MARKETFEETRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_MARKETFEETRK_202401010000.CSV
APPEND INTO TABLE MARKETFEETRK
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
MARKETFEEVERSIONNO FLOAT EXTERNAL,
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
AUTHORISEDBY CHAR(15),
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss",
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
