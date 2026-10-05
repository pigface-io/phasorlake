-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#FPP_FORECAST_RESIDUAL_DCF#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS FPP_FORECAST_RESIDUAL_DCF owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#FPP_FORECAST_RESIDUAL_DCF#FILE01#202608010000.CSV
APPEND INTO TABLE FPP_FORECAST_RESIDUAL_DCF
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
CONSTRAINTID CHAR(20),
EFFECTIVE_START_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
EFFECTIVE_END_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
VERSIONNO FLOAT EXTERNAL,
BIDTYPE CHAR(10),
RESIDUAL_DCF FLOAT EXTERNAL,
RESIDUAL_DCF_REASON_FLAG FLOAT EXTERNAL,
DCF_ABS_NEGATIVE_PERF_TOTAL FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
