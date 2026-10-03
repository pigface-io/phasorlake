-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#DEMANDOPERATIONALACTUAL#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS DEMANDOPERATIONALACTUAL owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#DEMANDOPERATIONALACTUAL#FILE01#202608010000.CSV
APPEND INTO TABLE DEMANDOPERATIONALACTUAL
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
REGIONID CHAR(20),
OPERATIONAL_DEMAND FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
OPERATIONAL_DEMAND_ADJUSTMENT FLOAT EXTERNAL,
WDR_ESTIMATE FLOAT EXTERNAL)
-- End of Script
