-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#ROOFTOP_PV_FORECAST#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS ROOFTOP_PV_FORECAST owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#ROOFTOP_PV_FORECAST#FILE01#202608010000.CSV
APPEND INTO TABLE ROOFTOP_PV_FORECAST
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
VERSION_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
REGIONID CHAR(20),
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
POWERMEAN FLOAT EXTERNAL,
POWERPOE50 FLOAT EXTERNAL,
POWERPOELOW FLOAT EXTERNAL,
POWERPOEHIGH FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
