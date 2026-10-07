-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#SETCFG_WDR_REIMBURSE_RATE#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS SETCFG_WDR_REIMBURSE_RATE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#SETCFG_WDR_REIMBURSE_RATE#FILE01#202408010000.CSV
APPEND INTO TABLE SETCFG_WDR_REIMBURSE_RATE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
WDRRRPERIOD CHAR(20),
REGIONID CHAR(20),
VERSION_DATETIME TIMESTAMP(3),
WDRRR FLOAT EXTERNAL,
ISFIRM FLOAT EXTERNAL,
LASTCHANGED TIMESTAMP(3))
-- End of Script
