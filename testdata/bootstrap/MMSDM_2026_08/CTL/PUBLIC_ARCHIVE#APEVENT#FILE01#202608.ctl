-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#APEVENT#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS APEVENT owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#APEVENT#FILE01#202608010000.CSV
APPEND INTO TABLE APEVENT
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
APEVENTID FLOAT EXTERNAL,
EFFECTIVEFROMINTERVAL DATE "YYYY/MM/DD HH24:MI:SS",
EFFECTIVETOINTERVAL DATE "YYYY/MM/DD HH24:MI:SS",
REASON CHAR(2000),
STARTAUTHORISEDBY CHAR(15),
STARTAUTHORISEDDATE DATE "YYYY/MM/DD HH24:MI:SS",
ENDAUTHORISEDBY CHAR(15),
ENDAUTHORISEDDATE DATE "YYYY/MM/DD HH24:MI:SS",
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
