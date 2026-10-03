-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#MTPASA_RESERVELIMIT_SET#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MTPASA_RESERVELIMIT_SET owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#MTPASA_RESERVELIMIT_SET#FILE01#202608010000.CSV
APPEND INTO TABLE MTPASA_RESERVELIMIT_SET
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
EFFECTIVEDATE DATE "YYYY/MM/DD HH24:MI:SS",
VERSION_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
RESERVELIMIT_SET_ID CHAR(20),
DESCRIPTION CHAR(200),
AUTHORISEDDATE DATE "YYYY/MM/DD HH24:MI:SS",
AUTHORISEDBY CHAR(20),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
