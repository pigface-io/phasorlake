-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#SET_NMAS_RECOVERY_RBF#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS SET_NMAS_RECOVERY_RBF owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#SET_NMAS_RECOVERY_RBF#FILE01#202408010000.CSV
APPEND INTO TABLE SET_NMAS_RECOVERY_RBF
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
VERSIONNO FLOAT EXTERNAL,
PERIODID FLOAT EXTERNAL,
SERVICE CHAR(10),
CONTRACTID CHAR(10),
PAYMENTTYPE CHAR(20),
REGIONID CHAR(10),
RBF FLOAT EXTERNAL,
PAYMENT_AMOUNT FLOAT EXTERNAL,
RECOVERY_AMOUNT FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
