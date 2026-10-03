-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#BILLING_NMAS_TST_RECVRY_TRK#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS BILLING_NMAS_TST_RECVRY_TRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#BILLING_NMAS_TST_RECVRY_TRK#FILE01#202608010000.CSV
APPEND INTO TABLE BILLING_NMAS_TST_RECVRY_TRK
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
CONTRACTYEAR FLOAT EXTERNAL,
WEEKNO FLOAT EXTERNAL,
BILLRUNNO FLOAT EXTERNAL,
RECOVERY_CONTRACTYEAR FLOAT EXTERNAL,
RECOVERY_WEEKNO FLOAT EXTERNAL,
RECOVERY_BILLRUNNO FLOAT EXTERNAL)
-- End of Script
