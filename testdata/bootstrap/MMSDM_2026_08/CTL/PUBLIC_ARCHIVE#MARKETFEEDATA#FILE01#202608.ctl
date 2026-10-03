-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#MARKETFEEDATA#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MARKETFEEDATA owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#MARKETFEEDATA#FILE01#202608010000.CSV
APPEND INTO TABLE MARKETFEEDATA
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
MARKETFEEID CHAR(10),
MARKETFEEVERSIONNO FLOAT EXTERNAL,
EFFECTIVEDATE DATE "YYYY/MM/DD HH24:MI:SS",
MARKETFEEVALUE FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
