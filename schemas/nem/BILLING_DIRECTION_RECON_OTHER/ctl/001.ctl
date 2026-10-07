-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#BILLING_DIRECTION_RECON_OTHER#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS BILLING_DIRECTION_RECON_OTHER owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#BILLING_DIRECTION_RECON_OTHER#FILE01#202408010000.CSV
APPEND INTO TABLE BILLING_DIRECTION_RECON_OTHER
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
DIRECTION_ID CHAR(20),
REGIONID CHAR(20),
DIRECTION_DESC CHAR(200),
DIRECTION_TYPE_ID CHAR(20),
DIRECTION_START_DATE DATE "YYYY/MM/DD HH24:MI:SS",
DIRECTION_END_DATE DATE "YYYY/MM/DD HH24:MI:SS",
DIRECTION_START_INTERVAL DATE "YYYY/MM/DD HH24:MI:SS",
DIRECTION_END_INTERVAL DATE "YYYY/MM/DD HH24:MI:SS",
COMPENSATION_AMOUNT FLOAT EXTERNAL,
INTEREST_AMOUNT FLOAT EXTERNAL,
INDEPENDENT_EXPERT_FEE FLOAT EXTERNAL,
CRA FLOAT EXTERNAL,
REGIONAL_CUSTOMER_ENERGY FLOAT EXTERNAL,
REGIONAL_GENERATOR_ENERGY FLOAT EXTERNAL,
REGIONAL_BENEFIT_FACTOR FLOAT EXTERNAL,
REGION_ACE_MWH FLOAT EXTERNAL,
REGION_ASOE_MWH FLOAT EXTERNAL,
DIRECTION_SERVICE_ID CHAR(20))
-- End of Script
