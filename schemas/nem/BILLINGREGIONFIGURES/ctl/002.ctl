-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#BILLINGREGIONFIGURES#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS BILLINGREGIONFIGURES owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#BILLINGREGIONFIGURES#FILE01#202608010000.CSV
APPEND INTO TABLE BILLINGREGIONFIGURES
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
REGIONID CHAR(10),
ENERGYOUT FLOAT EXTERNAL,
VALUEOUT FLOAT EXTERNAL,
ENERGYPURCHASED FLOAT EXTERNAL,
VALUEPURCHASED FLOAT EXTERNAL,
EXCESSGEN FLOAT EXTERNAL,
RESERVETRADING FLOAT EXTERNAL,
INTCOMPO FLOAT EXTERNAL,
ADMINPRICECOMPO FLOAT EXTERNAL,
SETTSURPLUS FLOAT EXTERNAL,
ASPAYMENT FLOAT EXTERNAL,
POOLFEES FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
WDRSQ FLOAT EXTERNAL,
WDRTA FLOAT EXTERNAL)
-- End of Script
