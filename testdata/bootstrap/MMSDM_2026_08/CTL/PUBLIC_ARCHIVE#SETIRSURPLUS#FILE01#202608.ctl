-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#SETIRSURPLUS#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS SETIRSURPLUS owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#SETIRSURPLUS#FILE01#202608010000.CSV
APPEND INTO TABLE SETIRSURPLUS
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
SETTLEMENTRUNNO FLOAT EXTERNAL,
PERIODID FLOAT EXTERNAL,
INTERCONNECTORID CHAR(10),
REGIONID CHAR(10),
MWFLOW FLOAT EXTERNAL,
LOSSFACTOR FLOAT EXTERNAL,
SURPLUSVALUE FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
CSP_DEROGATION_AMOUNT FLOAT EXTERNAL,
UNADJUSTED_IRSR FLOAT EXTERNAL)
-- End of Script
