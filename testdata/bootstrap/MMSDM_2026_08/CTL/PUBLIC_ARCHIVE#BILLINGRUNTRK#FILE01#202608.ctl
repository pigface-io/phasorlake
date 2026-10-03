-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#BILLINGRUNTRK#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS BILLINGRUNTRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#BILLINGRUNTRK#FILE01#202608010000.CSV
APPEND INTO TABLE BILLINGRUNTRK
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
STATUS CHAR(6),
ADJ_CLEARED CHAR(1),
AUTHORISEDDATE DATE "YYYY/MM/DD HH24:MI:SS",
AUTHORISEDBY CHAR(10),
POSTDATE DATE "YYYY/MM/DD HH24:MI:SS",
POSTBY CHAR(10),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
RECEIPTPOSTDATE DATE "YYYY/MM/DD HH24:MI:SS",
RECEIPTPOSTBY CHAR(10),
PAYMENTPOSTDATE DATE "YYYY/MM/DD HH24:MI:SS",
PAYMENTPOSTBY CHAR(10),
SHORTFALL FLOAT EXTERNAL,
MAKEUP FLOAT EXTERNAL,
REVISIONINDEX FLOAT EXTERNAL)
-- End of Script
