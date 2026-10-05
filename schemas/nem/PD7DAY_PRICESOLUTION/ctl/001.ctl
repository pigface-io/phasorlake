-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#PD7DAY_PRICESOLUTION#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS PD7DAY_PRICESOLUTION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#PD7DAY_PRICESOLUTION#FILE01#202608010000.CSV
APPEND INTO TABLE PD7DAY_PRICESOLUTION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
INTERVENTION FLOAT EXTERNAL,
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
REGIONID CHAR(20),
RRP FLOAT EXTERNAL,
LOWER1SECRRP FLOAT EXTERNAL,
LOWER6SECRRP FLOAT EXTERNAL,
LOWER60SECRRP FLOAT EXTERNAL,
LOWER5MINRRP FLOAT EXTERNAL,
LOWERREGRRP FLOAT EXTERNAL,
RAISE1SECRRP FLOAT EXTERNAL,
RAISE6SECRRP FLOAT EXTERNAL,
RAISE60SECRRP FLOAT EXTERNAL,
RAISE5MINRRP FLOAT EXTERNAL,
RAISEREGRRP FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
BDU_INITIAL_ENERGY_STORAGE FLOAT EXTERNAL,
DECGEN_INITIAL_ENERGY_STORAGE FLOAT EXTERNAL)
-- End of Script
