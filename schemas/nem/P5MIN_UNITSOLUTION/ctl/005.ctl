-- ************************************************************
-- Title:	PUBLIC_DVD_P5MIN_UNITSOLUTION_201910.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS P5MIN_UNITSOLUTION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_P5MIN_UNITSOLUTION_201910010000.CSV
APPEND INTO TABLE P5MIN_UNITSOLUTION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
INTERVENTION FLOAT EXTERNAL,
INTERVAL_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
DUID CHAR(10),
CONNECTIONPOINTID CHAR(12),
TRADETYPE FLOAT EXTERNAL,
AGCSTATUS FLOAT EXTERNAL,
INITIALMW FLOAT EXTERNAL,
TOTALCLEARED FLOAT EXTERNAL,
RAMPDOWNRATE FLOAT EXTERNAL,
RAMPUPRATE FLOAT EXTERNAL,
LOWER5MIN FLOAT EXTERNAL,
LOWER60SEC FLOAT EXTERNAL,
LOWER6SEC FLOAT EXTERNAL,
RAISE5MIN FLOAT EXTERNAL,
RAISE60SEC FLOAT EXTERNAL,
RAISE6SEC FLOAT EXTERNAL,
LOWERREG FLOAT EXTERNAL,
RAISEREG FLOAT EXTERNAL,
AVAILABILITY FLOAT EXTERNAL,
RAISE6SECFLAGS FLOAT EXTERNAL,
RAISE60SECFLAGS FLOAT EXTERNAL,
RAISE5MINFLAGS FLOAT EXTERNAL,
RAISEREGFLAGS FLOAT EXTERNAL,
LOWER6SECFLAGS FLOAT EXTERNAL,
LOWER60SECFLAGS FLOAT EXTERNAL,
LOWER5MINFLAGS FLOAT EXTERNAL,
LOWERREGFLAGS FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
SEMIDISPATCHCAP FLOAT EXTERNAL)
-- End of Script
