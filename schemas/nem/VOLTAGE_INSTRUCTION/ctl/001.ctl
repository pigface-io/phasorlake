-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#VOLTAGE_INSTRUCTION#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS VOLTAGE_INSTRUCTION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#VOLTAGE_INSTRUCTION#FILE01#202408010000.CSV
APPEND INTO TABLE VOLTAGE_INSTRUCTION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
EMS_ID CHAR(60),
PARTICIPANTID CHAR(20),
STATION_ID CHAR(60),
DEVICE_ID CHAR(60),
DEVICE_TYPE CHAR(20),
CONTROL_TYPE CHAR(20),
TARGET FLOAT EXTERNAL,
CONFORMING FLOAT EXTERNAL,
INSTRUCTION_SUMMARY CHAR(400),
VERSION_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
INSTRUCTION_SEQUENCE FLOAT EXTERNAL,
ADDITIONAL_NOTES CHAR(60))
-- End of Script
