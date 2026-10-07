-- ************************************************************
-- Title:	PUBLIC_DVD_ANCILLARY_RECOVERY_SPLIT_202405.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS ANCILLARY_RECOVERY_SPLIT owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_ANCILLARY_RECOVERY_SPLIT_202405010000.CSV
APPEND INTO TABLE ANCILLARY_RECOVERY_SPLIT
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
VERSIONNO FLOAT EXTERNAL,
SERVICE CHAR(10),
PAYMENTTYPE CHAR(20),
CUSTOMER_PORTION FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
ACE_PORTION FLOAT EXTERNAL)
-- End of Script
