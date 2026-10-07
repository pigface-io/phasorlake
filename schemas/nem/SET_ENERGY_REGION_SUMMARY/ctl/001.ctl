-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#SET_ENERGY_REGION_SUMMARY#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS SET_ENERGY_REGION_SUMMARY owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#SET_ENERGY_REGION_SUMMARY#FILE01#202408010000.CSV
APPEND INTO TABLE SET_ENERGY_REGION_SUMMARY
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
REGIONID CHAR(20),
CE_MWH FLOAT EXTERNAL,
UFEA_MWH FLOAT EXTERNAL,
ACE_MWH FLOAT EXTERNAL,
ASOE_MWH FLOAT EXTERNAL,
ACE_AMOUNT FLOAT EXTERNAL,
ASOE_AMOUNT FLOAT EXTERNAL,
TOTAL_MWH FLOAT EXTERNAL,
TOTAL_AMOUNT FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
