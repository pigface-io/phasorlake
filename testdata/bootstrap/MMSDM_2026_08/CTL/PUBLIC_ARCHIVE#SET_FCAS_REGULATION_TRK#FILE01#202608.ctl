-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#SET_FCAS_REGULATION_TRK#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS SET_FCAS_REGULATION_TRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#SET_FCAS_REGULATION_TRK#FILE01#202608010000.CSV
APPEND INTO TABLE SET_FCAS_REGULATION_TRK
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
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
CONSTRAINTID CHAR(20),
CMPF FLOAT EXTERNAL,
CRMPF FLOAT EXTERNAL,
RECOVERY_FACTOR_CMPF FLOAT EXTERNAL,
RECOVERY_FACTOR_CRMPF FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
USESUBSTITUTEDEMAND FLOAT EXTERNAL,
REQUIREMENTDEMAND FLOAT EXTERNAL,
FPP_AMOUNT FLOAT EXTERNAL,
FPP_RESIDUAL_AMOUNT FLOAT EXTERNAL,
USED_AMOUNT FLOAT EXTERNAL,
USED_RESIDUAL_AMOUNT FLOAT EXTERNAL,
UNUSED_AMOUNT FLOAT EXTERNAL,
UNUSED_RESIDUAL_AMOUNT FLOAT EXTERNAL,
P_REGULATION FLOAT EXTERNAL,
TSFCAS FLOAT EXTERNAL,
RCR FLOAT EXTERNAL,
USAGE_VALUE FLOAT EXTERNAL,
RCF FLOAT EXTERNAL,
NRCF FLOAT EXTERNAL,
DRCF FLOAT EXTERNAL,
RESIDUALTOTAL_MWH FLOAT EXTERNAL)
-- End of Script
