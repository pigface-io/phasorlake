-- ************************************************************
-- Title:	PUBLIC_DVD_MARKETFEE_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MARKETFEE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_MARKETFEE_202401010000.CSV
APPEND INTO TABLE MARKETFEE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
MARKETFEEID CHAR(10),
MARKETFEEPERIOD CHAR(20),
MARKETFEETYPE CHAR(12),
DESCRIPTION CHAR(64),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
GL_TCODE CHAR(15),
GL_FINANCIALCODE CHAR(10),
FEE_CLASS CHAR(40))
-- End of Script
