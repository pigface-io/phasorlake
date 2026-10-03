-- ************************************************************
-- Title:	PUBLIC_DVD_ROOFTOP_PV_FORECAST_202401.ctl
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
INFILE PUBLIC_DVD_ROOFTOP_PV_FORECAST_202401010000.CSV
APPEND INTO TABLE ROOFTOP_PV_FORECAST
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
VERSION_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
REGIONID CHAR(20),
INTERVAL_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
POWERMEAN FLOAT EXTERNAL,
POWERPOE50 FLOAT EXTERNAL,
POWERPOELOW FLOAT EXTERNAL,
POWERPOEHIGH FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
