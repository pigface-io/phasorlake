-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#GENERICEQUATIONRHS#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS GENERICEQUATIONRHS owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#GENERICEQUATIONRHS#FILE01#202608010000.CSV
APPEND INTO TABLE GENERICEQUATIONRHS
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
EQUATIONID CHAR(20),
EFFECTIVEDATE DATE "YYYY/MM/DD HH24:MI:SS",
VERSIONNO FLOAT EXTERNAL,
TERMID FLOAT EXTERNAL,
GROUPID FLOAT EXTERNAL,
SPD_ID CHAR(21),
SPD_TYPE CHAR(1),
FACTOR FLOAT EXTERNAL,
OPERATION CHAR(10),
DEFAULTVALUE FLOAT EXTERNAL,
PARAMETERTERM1 CHAR(12),
PARAMETERTERM2 CHAR(12),
PARAMETERTERM3 CHAR(12),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
