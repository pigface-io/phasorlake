-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#BILLING_CO2E_PUBLICATION#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS BILLING_CO2E_PUBLICATION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#BILLING_CO2E_PUBLICATION#FILE01#202608010000.CSV
APPEND INTO TABLE BILLING_CO2E_PUBLICATION
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
SETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
REGIONID CHAR(20),
SENTOUTENERGY FLOAT EXTERNAL,
GENERATOREMISSIONS FLOAT EXTERNAL,
INTENSITYINDEX FLOAT EXTERNAL)
-- End of Script
