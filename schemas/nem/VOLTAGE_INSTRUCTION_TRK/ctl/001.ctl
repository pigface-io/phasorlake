-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#VOLTAGE_INSTRUCTION_TRK#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS VOLTAGE_INSTRUCTION_TRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#VOLTAGE_INSTRUCTION_TRK#FILE01#202408010000.CSV
APPEND INTO TABLE VOLTAGE_INSTRUCTION_TRK
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
FILE_TYPE CHAR(20),
VERSION_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
SE_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
SOLUTION_CATEGORY CHAR(60),
SOLUTION_STATUS CHAR(60),
OPERATING_MODE CHAR(60),
OPERATING_STATUS CHAR(100),
EST_EXPIRY DATE "YYYY/MM/DD HH24:MI:SS",
EST_NEXT_INSTRUCTION DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
