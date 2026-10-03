-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#MTPASA_CONSTRAINTRESULT#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MTPASA_CONSTRAINTRESULT owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#MTPASA_CONSTRAINTRESULT#FILE01#202608010000.CSV
APPEND INTO TABLE MTPASA_CONSTRAINTRESULT
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
RUN_NO FLOAT EXTERNAL,
RUNTYPE CHAR(20),
DEMAND_POE_TYPE CHAR(20),
DAY DATE "YYYY/MM/DD HH24:MI:SS",
CONSTRAINTID CHAR(20),
EFFECTIVEDATE DATE "YYYY/MM/DD HH24:MI:SS",
VERSIONNO FLOAT EXTERNAL,
PERIODID FLOAT EXTERNAL,
PROBABILITYOFBINDING FLOAT EXTERNAL,
PROBABILITYOFVIOLATION FLOAT EXTERNAL,
CONSTRAINTVIOLATION90 FLOAT EXTERNAL,
CONSTRAINTVIOLATION50 FLOAT EXTERNAL,
CONSTRAINTVIOLATION10 FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
