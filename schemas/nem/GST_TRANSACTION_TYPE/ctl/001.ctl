-- ************************************************************
-- Title:	PUBLIC_DVD_GST_TRANSACTION_TYPE_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS GST_TRANSACTION_TYPE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_GST_TRANSACTION_TYPE_202401010000.CSV
APPEND INTO TABLE GST_TRANSACTION_TYPE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
TRANSACTION_TYPE CHAR(30),
DESCRIPTION CHAR(100),
GL_FINANCIALCODE CHAR(10),
GL_TCODE CHAR(15),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
