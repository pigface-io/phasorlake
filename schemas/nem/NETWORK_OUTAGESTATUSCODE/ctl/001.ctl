-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#NETWORK_OUTAGESTATUSCODE#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS NETWORK_OUTAGESTATUSCODE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#NETWORK_OUTAGESTATUSCODE#FILE01#202408010000.CSV
APPEND INTO TABLE NETWORK_OUTAGESTATUSCODE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
OUTAGESTATUSCODE CHAR(10),
DESCRIPTION CHAR(100),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
