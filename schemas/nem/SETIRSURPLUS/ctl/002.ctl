-- ************************************************************
-- Title:	PUBLIC_DVD_SETIRSURPLUS_201907.ctl
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
INFILE PUBLIC_DVD_SETIRSURPLUS_201907010000.CSV
APPEND INTO TABLE SETIRSURPLUS
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss",
SETTLEMENTRUNNO FLOAT EXTERNAL,
PERIODID FLOAT EXTERNAL,
INTERCONNECTORID CHAR(10),
REGIONID CHAR(10),
MWFLOW FLOAT EXTERNAL,
LOSSFACTOR FLOAT EXTERNAL,
SURPLUSVALUE FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
UNADJUSTED_IRSR FLOAT EXTERNAL,
CSP_DEROGATION_AMOUNT FLOAT EXTERNAL)
-- End of Script
