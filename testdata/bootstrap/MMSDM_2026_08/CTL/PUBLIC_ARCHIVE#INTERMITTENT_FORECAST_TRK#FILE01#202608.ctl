-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#INTERMITTENT_FORECAST_TRK#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS INTERMITTENT_FORECAST_TRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#INTERMITTENT_FORECAST_TRK#FILE01#202608010000.CSV
APPEND INTO TABLE INTERMITTENT_FORECAST_TRK
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
DUID CHAR(20),
ORIGIN CHAR(20),
FORECAST_PRIORITY FLOAT EXTERNAL,
OFFERDATETIME DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
