-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#FPP_RUN#FILE01#202412.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS FPP_RUN owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#FPP_RUN#FILE01#202412010000.CSV
APPEND INTO TABLE FPP_RUN
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
VERSIONNO FLOAT EXTERNAL,
FPPRUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
RUN_STATUS CHAR(20),
AUTHORISED_DATETIME DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
