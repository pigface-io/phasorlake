-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#BILLINGCALENDAR#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS BILLINGCALENDAR owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#BILLINGCALENDAR#FILE01#202608010000.CSV
APPEND INTO TABLE BILLINGCALENDAR
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
STARTDATE DATE "YYYY/MM/DD HH24:MI:SS",
ENDDATE DATE "YYYY/MM/DD HH24:MI:SS",
PRELIMINARYSTATEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
FINALSTATEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
PAYMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
REVISION1_STATEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
REVISION2_STATEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
REVISION0_STATEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
